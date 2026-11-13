// SPDX-License-Identifier: CC0-1.0

//! The CloudFront distribution lifecycle plus cache invalidation, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! restXml deserialization, the handler, the persisted state and serialization — the
//! same path a client takes. The rules under test are the ones CloudFront's
//! distribution surface states:
//!
//! * a distribution is PERSISTED by the call that creates or updates it, so every later
//!   read answers from that record: the configuration a read returns is the one that was
//!   submitted, with the members the service itself supplies resolved into it (see
//!   `distribution_config_defaults.rs` for that rule), and the id, ARN and domain name it
//!   reports never change;
//! * a distribution carries a VERSION token (`ETag`) that changes on every mutation and
//!   nothing else, and a mutation under `If-Match` lands only when the token it carries
//!   is the current one — a token that is not current fails the precondition, and a
//!   value that is no version at all is refused as an invalid `If-Match` version;
//! * `ListDistributions` cuts a page at `MaxItems`, hands back a `NextMarker` exactly
//!   when distributions remain, and resumes after the id the previous page ended at — so
//!   walking the pages at ANY admissible page size enumerates the whole collection once;
//! * every CloudFront list travels in a wrapper that states its own length, and a
//!   request whose `Quantity` disagrees with its `Items` is refused;
//! * a distribution or an invalidation CloudFront has just accepted is `InProgress`, and
//!   the clock — not a later call — owes it the terminal status;
//! * deleting a distribution requires its stored configuration to have it disabled AND
//!   that configuration to have finished propagating, and after the delete the
//!   distribution is gone from every read.
//!
//! The restXml bodies are read through the small element scanner below, so each
//! assertion is about the document a client would read rather than about a substring of
//! it. That scanner, the router harness and the `Config` table are `pub(crate)` so the
//! crate's other suites read responses the same way rather than each growing a copy.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    DEFAULT_PRICE_CLASS, MAX_INVALIDATION_PATHS, MAX_ITEMS, PROPAGATION_DELAY,
};
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service's signing name (`cloudfront`). It is
/// what routes a request to this service.
pub(crate) const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cloudfront/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The host every request below is addressed to, and so the authority a `Location`
/// header names.
pub(crate) const HOST: &str = "cloudfront.amazonaws.com";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`), spelled
/// out here so a changed ARN fails this suite.
pub(crate) const ACCOUNT_ID: &str = "000000000000";

/// This service's API version, which every route and `Location` is under.
pub(crate) const API_VERSION: &str = "2020-05-31";

/// One emulator: a router carrying only this service, over one shared engine.
///
/// `pub(crate)` so the sibling suites in `tests/all/` stand one up the same way rather
/// than each writing its own harness.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Long enough after a mutation that the clock owes the distribution its deployment.
///
/// `pub(crate)` and defined ONCE here: every suite that waits for a propagation waits for
/// the same span rather than carrying its own copy of it.
pub(crate) fn past_propagation() -> Duration {
    PROPAGATION_DELAY + Duration::from_millis(250)
}

/// How long a poll loop waits for a resource to reach the state the CLOCK owes it.
/// Generous: what a suite asserts is that the state arrives, never how fast the host is.
///
/// `pub(crate)` and defined ONCE here, beside [`past_propagation`], so every suite that
/// waits for a state waits under the same policy rather than carrying its own copy of it.
pub(crate) const READY_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll loop waits before looking again.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The status a distribution whose configuration has reached the edge locations is in.
pub(crate) const DEPLOYED: &str = "Deployed";

/// Look again until the distribution `id` reports itself [`DEPLOYED`] — a configuration
/// that has not reached the edge locations serves no viewer — or [`READY_DEADLINE`]
/// passes, which fails the case.
///
/// The CONDITION is what is waited for, never a span of the host's clock: the transition
/// is the one the clock owes the record, and the read is what materialises it.
pub(crate) async fn awaited_deployment(router: &MultiServiceRouter, case: &str, id: &str) {
    let deadline = Instant::now() + READY_DEADLINE;
    loop {
        let read = get_distribution(router, id).await;
        if read.root().find_text("Status").unwrap_or_default() == DEPLOYED {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: the distribution never reached {DEPLOYED}, last answer {}",
            read.raw
        );
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Whether `domain` is an address a client on the machine this emulator runs on can DIAL:
/// the `host:port` of the listener a distribution's edge is served on, which is what a
/// distribution publishes as its `DomainName`.
///
/// A name under CloudFront's own zone resolves to nothing here, so a distribution that
/// reported one would publish an address no viewer could reach; what it reports instead is
/// the authority of its own listener, and `SocketAddr` is exactly the set of values a client
/// can dial without resolving a name (an IPv6 literal included, which carries brackets).
///
/// `pub(crate)` so every suite that reads a published domain name holds it to the one rule.
pub(crate) fn is_reachable_authority(domain: &str) -> bool {
    domain.parse::<std::net::SocketAddr>().is_ok()
}

// ---------------------------------------------------------------------------
// XML reading
// ---------------------------------------------------------------------------

/// One element of a parsed response document: its local name, its text and its
/// children.
#[derive(Debug, Clone, Default)]
pub(crate) struct Node {
    name: String,
    pub(crate) text: String,
    pub(crate) children: Vec<Node>,
}

impl Node {
    /// The first child element called `name`.
    pub(crate) fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|child| child.name == name)
    }

    /// The element at a `/`-separated path of element names.
    pub(crate) fn at(&self, path: &str) -> Option<&Node> {
        path.split('/').try_fold(self, |node, part| node.child(part))
    }

    /// The text of the element at `path`.
    pub(crate) fn text_at(&self, path: &str) -> Option<&str> {
        self.at(path).map(|node| node.text.as_str())
    }

    /// This element's own text.
    pub(crate) fn text(&self) -> &str {
        self.text.as_str()
    }

    /// This element's local name, so a suite can assert the ORDER of an element's
    /// children and not only their presence.
    pub(crate) fn name(&self) -> &str {
        self.name.as_str()
    }

    /// Every child element called `name`.
    pub(crate) fn children_named(&self, name: &str) -> Vec<&Node> {
        self.children
            .iter()
            .filter(|child| child.name == name)
            .collect()
    }

    /// The text of every descendant element called `name`, depth first.
    pub(crate) fn find_text(&self, name: &str) -> Option<&str> {
        if self.name == name {
            return Some(self.text.as_str());
        }
        self.children.iter().find_map(|child| child.find_text(name))
    }

    /// The first descendant element called `name`, depth first.
    ///
    /// The same walk [`Node::find_text`] makes, answering with the ELEMENT rather than its
    /// text, so a suite that has to read several members of a nested element (the
    /// `RealtimeLogConfig` an operation's result wraps, say) locates it through this one
    /// walk rather than carrying its own copy.
    pub(crate) fn descendant(&self, name: &str) -> Option<&Node> {
        if self.name == name {
            return Some(self);
        }
        self.children
            .iter()
            .find_map(|child| child.descendant(name))
    }
}

/// Parse a restXml document into its element tree.
///
/// The only documents this reads are the ones this service itself serialises, so the
/// scanner covers exactly what the restXml serializer emits — elements with optional
/// attributes, self-closing elements, the XML declaration, and text carrying the five
/// predefined entities — and PANICS on anything else rather than guessing, so a
/// malformed body fails the test loudly instead of silently parsing as nothing. It is
/// written here, test-only, rather than pulled in as a parser dependency: this fix adds
/// nothing to the workspace lockfile, which every service crate shares.
fn parse_xml(body: &str) -> Node {
    let mut stack = vec![Node {
        name: "#document".to_string(),
        ..Node::default()
    }];
    let mut rest = body;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('<') {
            let end = after
                .find('>')
                .unwrap_or_else(|| panic!("unterminated element in the answer: {body}"));
            let tag = &after[..end];
            rest = &after[end + 1..];
            // The XML declaration, a comment or a processing instruction: no element.
            if tag.starts_with('?') || tag.starts_with('!') {
                continue;
            }
            if let Some(closing) = tag.strip_prefix('/') {
                let done = stack.pop().expect("an element to close");
                assert_eq!(
                    done.name,
                    local_name(closing.trim()),
                    "mismatched closing element in the answer: {body}"
                );
                stack
                    .last_mut()
                    .expect("the document node is never popped")
                    .children
                    .push(done);
                continue;
            }
            let self_closing = tag.ends_with('/');
            let opening = tag.trim_end_matches('/');
            let node = Node {
                name: local_name(opening.split_whitespace().next().unwrap_or(opening)),
                ..Node::default()
            };
            if self_closing {
                stack
                    .last_mut()
                    .expect("the document node is never popped")
                    .children
                    .push(node);
            } else {
                stack.push(node);
            }
        } else {
            let end = rest.find('<').unwrap_or(rest.len());
            let text = unescape_xml(&rest[..end]);
            if !text.trim().is_empty() {
                stack
                    .last_mut()
                    .expect("the document node is never popped")
                    .text
                    .push_str(&text);
            }
            rest = &rest[end..];
        }
    }
    assert_eq!(
        stack.len(),
        1,
        "the answer left elements unclosed: {body}"
    );
    stack.pop().expect("the document node")
}

/// Resolve the five entities XML predefines. `&amp;` is resolved LAST so a value that
/// carried a literal `&` is not unescaped twice.
fn unescape_xml(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// An element name without its namespace prefix.
fn local_name(raw: &str) -> String {
    match raw.split_once(':') {
        Some((_, local)) => local.to_string(),
        None => raw.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Calling the emulator
// ---------------------------------------------------------------------------

/// What the emulator answered: the status, the headers and the parsed body.
pub(crate) struct Answer {
    pub(crate) status: u16,
    headers: ::http::HeaderMap,
    pub(crate) raw: String,
    body: Node,
}

impl Answer {
    /// The document's root element, or an empty node when the answer carried no body.
    pub(crate) fn root(&self) -> &Node {
        self.body.children.first().unwrap_or(&self.body)
    }

    /// The wire error code (`<Code>` in the restXml error envelope), or `""`.
    pub(crate) fn error_code(&self) -> &str {
        self.root().find_text("Code").unwrap_or_default()
    }

    /// One response header's value, or `""`.
    pub(crate) fn header(&self, name: &str) -> &str {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
    }
}

/// Send one request through the router.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&str>,
    if_match: Option<&str>,
) -> Answer {
    let headers: Vec<(&str, &str)> = if_match
        .map(|version| vec![("if-match", version)])
        .unwrap_or_default();
    call_with_headers(router, method, uri, body, &headers).await
}

/// Send one request through the router, carrying `headers` beyond the ones every call
/// above carries.
///
/// The one request path, so a suite that needs a header this service binds an input
/// member to (`CopyDistribution`'s `Staging`, say) goes over the same wire path as the
/// rest rather than standing up a second harness.
pub(crate) async fn call_with_headers(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&str>,
    headers: &[(&str, &str)],
) -> Answer {
    let mut request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", HOST)
        .header("authorization", AUTHORIZATION);
    if body.is_some() {
        request = request.header("content-type", "application/xml");
    }
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request
        .body(Bytes::from(body.unwrap_or_default().to_string()))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let bytes = response.into_body();
    let raw = String::from_utf8_lossy(&bytes).to_string();
    let body = if raw.trim().is_empty() {
        Node::default()
    } else {
        parse_xml(&raw)
    };
    Answer {
        status,
        headers,
        raw,
        body,
    }
}

pub(crate) async fn create_distribution(router: &MultiServiceRouter, config: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/distribution"),
        Some(config),
        None,
    )
    .await
}

pub(crate) async fn get_distribution(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/distribution/{id}"),
        None,
        None,
    )
    .await
}

pub(crate) async fn update_distribution(
    router: &MultiServiceRouter,
    id: &str,
    config: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/distribution/{id}/config"),
        Some(config),
        if_match,
    )
    .await
}

pub(crate) async fn delete_distribution(
    router: &MultiServiceRouter,
    id: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/{API_VERSION}/distribution/{id}"),
        None,
        if_match,
    )
    .await
}

pub(crate) async fn list_distributions(router: &MultiServiceRouter, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/distribution{query}"),
        None,
        None,
    )
    .await
}

pub(crate) async fn create_invalidation(
    router: &MultiServiceRouter,
    distribution_id: &str,
    batch: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/distribution/{distribution_id}/invalidation"),
        Some(batch),
        None,
    )
    .await
}

pub(crate) async fn get_invalidation(
    router: &MultiServiceRouter,
    distribution_id: &str,
    invalidation_id: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/distribution/{distribution_id}/invalidation/{invalidation_id}"),
        None,
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// The configurations under test
// ---------------------------------------------------------------------------

/// One distribution configuration a client submits.
///
/// The rows built from this differ along everything the rules generalise over: how many
/// origins the configuration carries, how many alternate domain names, whether it is
/// enabled, which viewer protocol policy its default cache behaviour names, and whether
/// it names optional members (`PriceClass`) at all.
#[derive(Debug, Clone)]
pub(crate) struct Config {
    pub(crate) case: &'static str,
    pub(crate) caller_reference: String,
    pub(crate) comment: String,
    pub(crate) enabled: bool,
    /// `(Id, DomainName)` of each origin, in order.
    pub(crate) origins: Vec<(String, String)>,
    /// `Aliases.Items`, the alternate domain names.
    pub(crate) aliases: Vec<String>,
    /// One of `ViewerProtocolPolicy`'s values (`allow-all` | `https-only` |
    /// `redirect-to-https`), from the model's enum.
    pub(crate) viewer_protocol_policy: &'static str,
    /// One of `PriceClass`'s values, or `None` to leave the member out entirely (the
    /// service then resolves it).
    pub(crate) price_class: Option<&'static str>,
    /// `DistributionConfig$WebACLId` — the WAF web ACL this configuration associates the
    /// distribution with, or `None` to leave the member out entirely (the service then
    /// stores no association).
    pub(crate) web_acl_id: Option<String>,
}

impl Config {
    /// The `DistributionConfig` XML a client sends for this configuration.
    pub(crate) fn xml(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
        ));
        out.push_str(&format!(
            "<CallerReference>{}</CallerReference>",
            self.caller_reference
        ));
        if !self.aliases.is_empty() {
            // `AliasList`'s member is named `CNAME` in XML.
            out.push_str(&format!(
                "<Aliases><Quantity>{}</Quantity><Items>",
                self.aliases.len()
            ));
            for alias in &self.aliases {
                out.push_str(&format!("<CNAME>{alias}</CNAME>"));
            }
            out.push_str("</Items></Aliases>");
        }
        out.push_str(&format!(
            "<Origins><Quantity>{}</Quantity><Items>",
            self.origins.len()
        ));
        for (id, domain) in &self.origins {
            out.push_str(&format!(
                "<Origin><Id>{id}</Id><DomainName>{domain}</DomainName>\
                 <CustomOriginConfig><HTTPPort>80</HTTPPort><HTTPSPort>443</HTTPSPort>\
                 <OriginProtocolPolicy>https-only</OriginProtocolPolicy>\
                 </CustomOriginConfig></Origin>"
            ));
        }
        out.push_str("</Items></Origins>");
        out.push_str(&format!(
            "<DefaultCacheBehavior><TargetOriginId>{}</TargetOriginId>\
             <ViewerProtocolPolicy>{}</ViewerProtocolPolicy></DefaultCacheBehavior>",
            self.origins
                .first()
                .map(|(id, _)| id.as_str())
                .unwrap_or_default(),
            self.viewer_protocol_policy
        ));
        out.push_str(&format!("<Comment>{}</Comment>", self.comment));
        if let Some(price_class) = self.price_class {
            out.push_str(&format!("<PriceClass>{price_class}</PriceClass>"));
        }
        out.push_str(&format!("<Enabled>{}</Enabled>", self.enabled));
        // `ViewerCertificate` follows `Enabled`: a configuration that declares an
        // alternate domain name has to name a custom certificate that covers it.
        if !self.aliases.is_empty() {
            out.push_str(CUSTOM_VIEWER_CERTIFICATE);
        }
        // Then `WebACLId`, which is where `DistributionConfig` carries it.
        if let Some(web_acl_id) = &self.web_acl_id {
            out.push_str(&format!("<WebACLId>{web_acl_id}</WebACLId>"));
        }
        out.push_str("</DistributionConfig>");
        out
    }

    /// The `DistributionConfig` XML a client sends to UPDATE a distribution to this
    /// configuration.
    ///
    /// An update submits the configuration WHOLE, so the document states the price class
    /// even where the row leaves the member out of the create: a configuration that names
    /// none is complete for a create, which resolves it, and incomplete for an update, which
    /// is refused.
    pub(crate) fn update_xml(&self) -> String {
        Config {
            price_class: Some(self.price_class.unwrap_or(DEFAULT_PRICE_CLASS)),
            ..self.clone()
        }
        .xml()
    }

    /// Assert that `config` — a `DistributionConfig` element the emulator answered with
    /// — is the configuration this row submitted.
    pub(crate) fn assert_matches(&self, config: &Node, context: &str) {
        let case = self.case;
        assert_eq!(
            config.text_at("CallerReference"),
            Some(self.caller_reference.as_str()),
            "case {case} ({context}): expected CallerReference {:?}, got {:?}",
            self.caller_reference,
            config.text_at("CallerReference")
        );
        assert_eq!(
            config.text_at("Comment"),
            Some(self.comment.as_str()),
            "case {case} ({context}): expected Comment {:?}, got {:?}",
            self.comment,
            config.text_at("Comment")
        );
        assert_eq!(
            config.text_at("Enabled"),
            Some(if self.enabled { "true" } else { "false" }),
            "case {case} ({context}): expected Enabled {}, got {:?}",
            self.enabled,
            config.text_at("Enabled")
        );
        assert_eq!(
            config.text_at("Origins/Quantity"),
            Some(self.origins.len().to_string().as_str()),
            "case {case} ({context}): expected Origins.Quantity {}, got {:?}",
            self.origins.len(),
            config.text_at("Origins/Quantity")
        );
        let origins: Vec<(String, String)> = config
            .at("Origins/Items")
            .map(|items| {
                items
                    .children_named("Origin")
                    .into_iter()
                    .map(|origin| {
                        (
                            origin.text_at("Id").unwrap_or_default().to_string(),
                            origin.text_at("DomainName").unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            origins, self.origins,
            "case {case} ({context}): expected origins {:?}, got {origins:?}",
            self.origins
        );
        assert_eq!(
            config.text_at("DefaultCacheBehavior/TargetOriginId"),
            self.origins.first().map(|(id, _)| id.as_str()),
            "case {case} ({context}): expected the default cache behaviour to target {:?}, got {:?}",
            self.origins.first().map(|(id, _)| id.as_str()),
            config.text_at("DefaultCacheBehavior/TargetOriginId")
        );
        assert_eq!(
            config.text_at("DefaultCacheBehavior/ViewerProtocolPolicy"),
            Some(self.viewer_protocol_policy),
            "case {case} ({context}): expected ViewerProtocolPolicy {:?}, got {:?}",
            self.viewer_protocol_policy,
            config.text_at("DefaultCacheBehavior/ViewerProtocolPolicy")
        );
        let aliases: Vec<String> = config
            .at("Aliases/Items")
            .map(|items| {
                items
                    .children_named("CNAME")
                    .into_iter()
                    .map(|alias| alias.text.clone())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            aliases, self.aliases,
            "case {case} ({context}): expected aliases {:?}, got {aliases:?}",
            self.aliases
        );
        // A member the request left out is reported with the value the SERVICE applies:
        // the stored configuration is the resolved one, so an unnamed `PriceClass` reads
        // back as the class that covers every edge location.
        let expected_price_class = self.price_class.unwrap_or(DEFAULT_PRICE_CLASS);
        assert_eq!(
            config.text_at("PriceClass"),
            Some(expected_price_class),
            "case {case} ({context}): expected PriceClass {expected_price_class:?}, got {:?}",
            config.text_at("PriceClass")
        );
        // `WebACLId` the same way: a configuration that named no web ACL reads back as the
        // empty value, which is what "associated with no web ACL" is.
        let expected_web_acl_id = self.web_acl_id.clone().unwrap_or_default();
        let reported_web_acl_id = config.text_at("WebACLId").unwrap_or_default();
        assert_eq!(
            reported_web_acl_id, expected_web_acl_id,
            "case {case} ({context}): expected WebACLId {expected_web_acl_id:?}, got \
             {reported_web_acl_id:?}"
        );
    }
}

/// The `ViewerCertificate` a configuration that declares an alternate domain name has to
/// carry.
///
/// `ViewerCertificate` states it: a distribution that uses `Aliases` names one of
/// `ACMCertificateArn` or `IAMCertificateId`, because CloudFront's own default certificate
/// validates no domain name of the caller's — so a row of the table below that states an
/// alias states a certificate too, which is what makes it a request CloudFront accepts.
pub(crate) const CUSTOM_VIEWER_CERTIFICATE: &str = "<ViewerCertificate>\
     <ACMCertificateArn>arn:aws:acm:us-east-1:000000000000:certificate/\
     11111111-2222-3333-4444-555555555555</ACMCertificateArn>\
     <SSLSupportMethod>sni-only</SSLSupportMethod>\
     <MinimumProtocolVersion>TLSv1.2_2021</MinimumProtocolVersion></ViewerCertificate>";

/// A configuration for `reference`, varying along the members the rules generalise over.
pub(crate) fn config(case: &'static str, reference: &str) -> Config {
    Config {
        case,
        caller_reference: reference.to_string(),
        comment: format!("distribution for {reference}"),
        enabled: true,
        origins: vec![(
            "primary".to_string(),
            "origin.example.com".to_string(),
        )],
        aliases: Vec::new(),
        viewer_protocol_policy: "allow-all",
        price_class: None,
        web_acl_id: None,
    }
}

/// The table every persistence assertion runs over: four configurations differing in
/// origin count, alias count, enabled flag, viewer protocol policy, comment and whether
/// the optional `PriceClass` is named at all.
pub(crate) fn config_table() -> Vec<Config> {
    vec![
        config("one-origin-no-aliases", "ref-minimal"),
        Config {
            aliases: vec!["cdn.example.com".to_string()],
            price_class: Some("PriceClass_100"),
            viewer_protocol_policy: "redirect-to-https",
            ..config("one-origin-one-alias", "ref-aliased")
        },
        Config {
            origins: vec![
                ("primary".to_string(), "a.example.com".to_string()),
                ("secondary".to_string(), "b.example.com".to_string()),
                ("tertiary".to_string(), "c.example.com".to_string()),
            ],
            aliases: vec![
                "one.example.com".to_string(),
                "two.example.com".to_string(),
            ],
            viewer_protocol_policy: "https-only",
            price_class: Some("PriceClass_All"),
            ..config("three-origins-two-aliases", "ref-multi")
        },
        Config {
            enabled: false,
            comment: String::new(),
            ..config("disabled-empty-comment", "ref-disabled")
        },
    ]
}

/// An `InvalidationBatch` XML body for `paths` under `reference`, stating the quantity
/// `paths` really has.
pub(crate) fn batch_xml(reference: &str, paths: &[&str]) -> String {
    batch_xml_with_quantity(reference, paths, paths.len() as i32)
}

/// An `InvalidationBatch` XML body that states `quantity` whatever `paths` carries, so a
/// test can submit a wrapper whose `Quantity` disagrees with its `Items`.
///
/// `pub(crate)` so every suite that submits a batch — against a distribution or against a
/// distribution TENANT — builds it here rather than carrying its own copy.
pub(crate) fn batch_xml_with_quantity(reference: &str, paths: &[&str], quantity: i32) -> String {
    let mut out = format!(
        "<InvalidationBatch xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Paths><Quantity>{quantity}</Quantity><Items>"
    );
    for path in paths {
        out.push_str(&format!("<Path>{path}</Path>"));
    }
    out.push_str(&format!(
        "</Items></Paths><CallerReference>{reference}</CallerReference></InvalidationBatch>"
    ));
    out
}

/// The sentence an `InconsistentQuantities` refusal carries for one list wrapper, built
/// from the request: the wrapper, the `Quantity` the request stated and the number of items
/// it really carried.
///
/// `pub(crate)` and written once here so every suite that asserts this refusal's wording
/// expects the same sentence rather than carrying its own spelling of it.
pub(crate) fn inconsistent_quantities_message(
    wrapper: &str,
    stated: i32,
    carried: usize,
) -> String {
    format!(
        "The specified quantity of {wrapper} ({stated}) does not match the actual quantity \
         supplied ({carried})."
    )
}

/// `Distribution$Id`'s shape: `E` followed by 13 uppercase alphanumerics, checked here
/// rather than through the crate's own predicate so a wrong predicate cannot make this
/// test pass.
pub(crate) fn is_distribution_id(value: &str) -> bool {
    match value.strip_prefix('E') {
        Some(body) => {
            body.len() == 13
                && body
                    .chars()
                    .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        }
        None => false,
    }
}

/// An invalidation id's shape: `I` followed by 13 uppercase alphanumerics.
pub(crate) fn is_invalidation_id(value: &str) -> bool {
    match value.strip_prefix('I') {
        Some(body) => {
            body.len() == 13
                && body
                    .chars()
                    .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        }
        None => false,
    }
}

/// A version token's shape: `E` followed by 13 uppercase alphanumerics, the shape
/// CloudFront's own `ETag` values take.
pub(crate) fn is_version_token(value: &str) -> bool {
    is_distribution_id(value)
}

/// The shape of every id CloudFront mints as a UUID — a key value store, a cache,
/// origin request, response headers or continuous deployment policy: five lowercase-hex
/// groups of 8-4-4-4-12 characters. One predicate here, so each suite that reads such an
/// id asserts the SAME shape rather than carrying its own copy.
pub(crate) fn is_minted_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8usize, 4, 4, 4, 12])
            .all(|(group, width)| {
                group.len() == width
                    && group
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            })
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// A create persists the submitted configuration, and every later read answers from
/// that record: the configuration comes back as submitted, and the identity CloudFront
/// allocated for it (id, ARN, domain name) is the same on every read.
#[tokio::test]
async fn create_persists_the_configuration_every_read_then_reports() {
    let router = emulator();
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();
    let mut seen_domains: BTreeSet<String> = BTreeSet::new();

    for row in config_table() {
        let case = row.case;
        let created = create_distribution(&router, &row.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            created.status, created.raw
        );
        let distribution = created.root();
        assert_eq!(
            distribution.name, "Distribution",
            "case {case}: expected a <Distribution> payload, got <{}>",
            distribution.name
        );

        let id = distribution.text_at("Id").unwrap_or_default().to_string();
        assert!(
            is_distribution_id(&id),
            "case {case}: expected an `E`+13 distribution id, got {id:?}"
        );
        assert!(
            seen_ids.insert(id.clone()),
            "case {case}: id {id} was already allocated to another distribution"
        );

        let expected_arn = format!("arn:aws:cloudfront::{ACCOUNT_ID}:distribution/{id}");
        assert_eq!(
            distribution.text_at("ARN"),
            Some(expected_arn.as_str()),
            "case {case}: expected ARN {expected_arn}, got {:?}",
            distribution.text_at("ARN")
        );

        let domain = distribution
            .text_at("DomainName")
            .unwrap_or_default()
            .to_string();
        assert!(
            is_reachable_authority(&domain),
            "case {case}: expected the address of the listener this distribution's edge is \
             served on, got {domain:?}"
        );
        assert!(
            seen_domains.insert(domain.clone()),
            "case {case}: domain {domain} was already allocated to another distribution"
        );

        // A distribution CloudFront has just accepted has not propagated yet.
        assert_eq!(
            distribution.text_at("Status"),
            Some("InProgress"),
            "case {case}: expected a just-created distribution to be InProgress, got {:?}",
            distribution.text_at("Status")
        );
        assert_eq!(
            distribution.text_at("InProgressInvalidationBatches"),
            Some("0"),
            "case {case}: expected no invalidation batches in progress, got {:?}",
            distribution.text_at("InProgressInvalidationBatches")
        );
        assert!(
            distribution
                .text_at("LastModifiedTime")
                .is_some_and(|value| !value.is_empty()),
            "case {case}: expected a LastModifiedTime, got {:?}",
            distribution.text_at("LastModifiedTime")
        );

        let etag = created.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {case}: expected an `E`+13 ETag header, got {etag:?}"
        );
        assert_eq!(
            created.header("location"),
            format!("https://{HOST}/{API_VERSION}/distribution/{id}"),
            "case {case}: expected the Location of the new distribution, got {:?}",
            created.header("location")
        );

        let config_node = distribution
            .child("DistributionConfig")
            .unwrap_or_else(|| panic!("case {case}: the answer carried no DistributionConfig"));
        row.assert_matches(config_node, "CreateDistribution");

        // The read answers from the stored record, not from the request that created it.
        let fetched = get_distribution(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected HTTP 200 from GetDistribution, got {} with {}",
            fetched.status, fetched.raw
        );
        assert_eq!(
            fetched.header("etag"),
            etag,
            "case {case}: expected GetDistribution to report the ETag the create handed back ({etag}), got {:?}",
            fetched.header("etag")
        );
        let fetched_distribution = fetched.root();
        for member in ["Id", "ARN", "DomainName"] {
            assert_eq!(
                fetched_distribution.text_at(member),
                distribution.text_at(member),
                "case {case}: expected GetDistribution to report the same {member} as the create, got {:?}",
                fetched_distribution.text_at(member)
            );
        }
        let fetched_config = fetched_distribution
            .child("DistributionConfig")
            .unwrap_or_else(|| panic!("case {case}: GetDistribution carried no DistributionConfig"));
        row.assert_matches(fetched_config, "GetDistribution");
    }

    // An unknown id names no distribution.
    let missing = get_distribution(&router, "E00000000000X").await;
    assert_eq!(
        missing.status, 404,
        "case unknown-id: expected HTTP 404, got {} with {}",
        missing.status, missing.raw
    );
    assert_eq!(
        missing.error_code(),
        "NoSuchDistribution",
        "case unknown-id: expected NoSuchDistribution, got {:?}",
        missing.error_code()
    );
}

/// A caller reference identifies one distribution: a second create under a reference the
/// store already holds is refused.
#[tokio::test]
async fn a_caller_reference_names_at_most_one_distribution() {
    let router = emulator();
    let row = config("duplicate-reference", "ref-once");

    let first = create_distribution(&router, &row.xml()).await;
    assert_eq!(
        first.status, 201,
        "case first-create: expected HTTP 201, got {} with {}",
        first.status, first.raw
    );

    // Same reference, different configuration: still the reference of the first one.
    let again = Config {
        comment: "a different configuration".to_string(),
        ..row.clone()
    };
    let second = create_distribution(&router, &again.xml()).await;
    assert_eq!(
        second.status, 409,
        "case repeat-create: expected HTTP 409, got {} with {}",
        second.status, second.raw
    );
    assert_eq!(
        second.error_code(),
        "DistributionAlreadyExists",
        "case repeat-create: expected DistributionAlreadyExists, got {:?}",
        second.error_code()
    );
}

/// Every mutation mints a fresh version token, and `If-Match` governs the mutation: the
/// current token lets it land, a token that is no longer current fails the precondition,
/// and a value that is no version token at all is an invalid `If-Match` version.
#[tokio::test]
async fn each_mutation_mints_a_version_and_if_match_governs_it() {
    let router = emulator();

    for row in config_table() {
        let case = row.case;
        let created = create_distribution(&router, &row.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            created.status, created.raw
        );
        let id = created
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string();
        let first_etag = created.header("etag").to_string();

        // Every If-Match value that is not the current version is refused, and refusing
        // it changes nothing.
        let refusals: &[(&str, Option<&str>, u16, &str)] = &[
            ("missing", None, 400, "InvalidIfMatchVersion"),
            ("not-a-version", Some("nonsense"), 400, "InvalidIfMatchVersion"),
            ("too-short", Some("E2QWRUHAPOMQ"), 400, "InvalidIfMatchVersion"),
            (
                "another-version",
                Some("EZZZZZZZZZZZZZ"),
                412,
                "PreconditionFailed",
            ),
        ];
        let changed = Config {
            enabled: !row.enabled,
            comment: "updated".to_string(),
            ..row.clone()
        };
        for (label, if_match, status, code) in refusals {
            let refused = update_distribution(&router, &id, &changed.update_xml(), *if_match).await;
            assert_eq!(
                refused.status, *status,
                "case {case}/{label}: expected HTTP {status}, got {} with {}",
                refused.status, refused.raw
            );
            assert_eq!(
                refused.error_code(),
                *code,
                "case {case}/{label}: expected {code}, got {:?}",
                refused.error_code()
            );
            let unchanged = get_distribution(&router, &id).await;
            assert_eq!(
                unchanged.header("etag"),
                first_etag,
                "case {case}/{label}: a refused update must not change the version"
            );
            row.assert_matches(
                unchanged
                    .root()
                    .child("DistributionConfig")
                    .unwrap_or_else(|| panic!("case {case}/{label}: no DistributionConfig")),
                "after a refused update",
            );
        }

        // The current version lets the update land, and mints a new one.
        let updated =
            update_distribution(&router, &id, &changed.update_xml(), Some(&first_etag)).await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected HTTP 200, got {} with {}",
            updated.status, updated.raw
        );
        let second_etag = updated.header("etag").to_string();
        assert!(
            is_version_token(&second_etag),
            "case {case}: expected an `E`+13 ETag, got {second_etag:?}"
        );
        assert_ne!(
            second_etag, first_etag,
            "case {case}: a mutation must mint a FRESH version, got {second_etag} twice"
        );
        changed.assert_matches(
            updated
                .root()
                .child("DistributionConfig")
                .unwrap_or_else(|| panic!("case {case}: the update carried no DistributionConfig")),
            "UpdateDistribution",
        );

        // The change is what a later read observes, under the new version.
        let fetched = get_distribution(&router, &id).await;
        assert_eq!(
            fetched.header("etag"),
            second_etag,
            "case {case}: expected the new version from GetDistribution, got {:?}",
            fetched.header("etag")
        );
        changed.assert_matches(
            fetched
                .root()
                .child("DistributionConfig")
                .unwrap_or_else(|| panic!("case {case}: no DistributionConfig")),
            "GetDistribution after the update",
        );

        // And the version it replaced no longer satisfies the precondition.
        let stale =
            update_distribution(&router, &id, &changed.update_xml(), Some(&first_etag)).await;
        assert_eq!(
            stale.status, 412,
            "case {case}: expected HTTP 412 for the replaced version, got {} with {}",
            stale.status, stale.raw
        );
        assert_eq!(
            stale.error_code(),
            "PreconditionFailed",
            "case {case}: expected PreconditionFailed, got {:?}",
            stale.error_code()
        );
    }
}

/// An update cannot rename a distribution's caller reference, and no update lands on a
/// distribution that does not exist.
#[tokio::test]
async fn update_refuses_a_renamed_caller_reference_and_an_unknown_id() {
    let router = emulator();
    let row = config("rename-reference", "ref-stable");
    let created = create_distribution(&router, &row.xml()).await;
    let id = created
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();
    let etag = created.header("etag").to_string();

    let renamed = Config {
        caller_reference: "ref-renamed".to_string(),
        ..row.clone()
    };
    let refused = update_distribution(&router, &id, &renamed.update_xml(), Some(&etag)).await;
    assert_eq!(
        refused.status, 400,
        "case renamed-reference: expected HTTP 400, got {} with {}",
        refused.status, refused.raw
    );
    assert_eq!(
        refused.error_code(),
        "IllegalUpdate",
        "case renamed-reference: expected IllegalUpdate, got {:?}",
        refused.error_code()
    );

    let missing =
        update_distribution(&router, "E00000000000X", &row.update_xml(), Some(&etag)).await;
    assert_eq!(
        missing.status, 404,
        "case unknown-id: expected HTTP 404, got {} with {}",
        missing.status, missing.raw
    );
    assert_eq!(
        missing.error_code(),
        "NoSuchDistribution",
        "case unknown-id: expected NoSuchDistribution, got {:?}",
        missing.error_code()
    );
}

/// `ListDistributions` enumerates the persisted distributions: a page holds at most
/// `MaxItems` summaries, `Quantity` is the page's own length, `NextMarker` arrives
/// exactly when more remain, and the next page resumes after the id this one ended at —
/// at every admissible page size.
#[tokio::test]
async fn list_pages_at_max_items_and_resumes_from_the_marker() {
    let router = emulator();
    let mut created: Vec<(String, String)> = Vec::new();
    for index in 0..5 {
        let row = config("listing", &format!("ref-list-{index}"));
        let answer = create_distribution(&router, &row.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case listing-{index}: expected HTTP 201, got {} with {}",
            answer.status, answer.raw
        );
        created.push((
            answer.root().text_at("Id").unwrap_or_default().to_string(),
            answer.header("etag").to_string(),
        ));
    }
    let mut expected_ids: Vec<String> = created.iter().map(|(id, _)| id.clone()).collect();
    expected_ids.sort();

    // Every admissible page size, plus the request that names none at all.
    let page_sizes: Vec<Option<i32>> = vec![None, Some(1), Some(2), Some(3), Some(4), Some(5), Some(50)];
    for page_size in page_sizes {
        let label = match page_size {
            None => "default".to_string(),
            Some(size) => format!("MaxItems={size}"),
        };
        let mut collected: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        for page in 0..=expected_ids.len() {
            let query = match (page_size, marker.as_deref()) {
                (None, None) => String::new(),
                (None, Some(value)) => format!("?Marker={value}"),
                (Some(size), None) => format!("?MaxItems={size}"),
                (Some(size), Some(value)) => format!("?MaxItems={size}&Marker={value}"),
            };
            let answer = list_distributions(&router, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {label} page {page}: expected HTTP 200, got {} with {}",
                answer.status, answer.raw
            );
            let list = answer.root();
            assert_eq!(
                list.name, "DistributionList",
                "case {label} page {page}: expected a <DistributionList> payload, got <{}>",
                list.name
            );
            assert_eq!(
                list.text_at("Marker"),
                Some(marker.as_deref().unwrap_or_default()),
                "case {label} page {page}: expected the Marker the request carried, got {:?}",
                list.text_at("Marker")
            );
            assert_eq!(
                list.text_at("MaxItems"),
                Some(page_size.unwrap_or(100).to_string().as_str()),
                "case {label} page {page}: expected MaxItems to echo the page size, got {:?}",
                list.text_at("MaxItems")
            );

            let items: Vec<&Node> = list
                .child("Items")
                .map(|items| items.children_named("DistributionSummary"))
                .unwrap_or_default();
            assert_eq!(
                list.text_at("Quantity"),
                Some(items.len().to_string().as_str()),
                "case {label} page {page}: expected Quantity to equal the {} items served, got {:?}",
                items.len(),
                list.text_at("Quantity")
            );
            if let Some(size) = page_size {
                assert!(
                    i32::try_from(items.len()).unwrap_or(i32::MAX) <= size,
                    "case {label} page {page}: a page must hold at most {size} summaries, got {}",
                    items.len()
                );
            }
            for summary in &items {
                let id = summary.text_at("Id").unwrap_or_default().to_string();
                let etag = summary.text_at("ETag").unwrap_or_default().to_string();
                let expected_etag = created
                    .iter()
                    .find(|(created_id, _)| *created_id == id)
                    .map(|(_, created_etag)| created_etag.clone())
                    .unwrap_or_default();
                assert_eq!(
                    etag, expected_etag,
                    "case {label} page {page}: expected summary {id} to carry the version its \
                     create handed back ({expected_etag}), got {etag:?}"
                );
                assert_eq!(
                    summary.text_at("ARN"),
                    Some(format!("arn:aws:cloudfront::{ACCOUNT_ID}:distribution/{id}").as_str()),
                    "case {label} page {page}: expected summary {id} to carry its ARN, got {:?}",
                    summary.text_at("ARN")
                );
                collected.push(id);
            }

            let next_marker = list.text_at("NextMarker").map(str::to_string);
            let is_truncated = list.text_at("IsTruncated").unwrap_or_default();
            match &next_marker {
                Some(next) => {
                    assert_eq!(
                        is_truncated, "true",
                        "case {label} page {page}: a NextMarker must come with IsTruncated true"
                    );
                    assert_eq!(
                        Some(next.as_str()),
                        collected.last().map(String::as_str),
                        "case {label} page {page}: NextMarker must name the id this page ended at"
                    );
                    marker = next_marker;
                }
                None => {
                    assert_eq!(
                        is_truncated, "false",
                        "case {label} page {page}: no NextMarker means IsTruncated false"
                    );
                    break;
                }
            }
        }
        assert_eq!(
            collected, expected_ids,
            "case {label}: walking the pages must enumerate every distribution exactly once, \
             in id order"
        );
    }

    // A marker this service never issued, and a page size that names no page, are
    // refused as invalid arguments.
    let refusals: &[(&str, &str)] = &[
        ("?Marker=not-an-id", "a marker that is no distribution id"),
        ("?MaxItems=-3", "a negative page size"),
    ];
    for (query, description) in refusals {
        let answer = list_distributions(&router, query).await;
        assert_eq!(
            answer.status, 400,
            "case {description}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {description}: expected InvalidArgument, got {:?}",
            answer.error_code()
        );
    }

    // `MaxItems` bounds the page rather than demanding one, so a page size of zero is a
    // page: an empty one, reported as truncated because distributions remain.
    let answer = list_distributions(&router, "?MaxItems=0").await;
    assert_eq!(
        answer.status, 200,
        "case a page size of zero: expected HTTP 200, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.root().text_at("Quantity"),
        Some("0"),
        "case a page size of zero: expected an empty page, got {}",
        answer.raw
    );
    assert_eq!(
        answer.root().text_at("IsTruncated"),
        Some("true"),
        "case a page size of zero: expected a truncated page, got {}",
        answer.raw
    );
}

/// A page is bounded by the SERVER's own ceiling, whatever `MaxItems` a caller asks
/// for: one request can never serve the whole store, and the rest is paged with
/// `Marker`.
#[tokio::test]
async fn a_page_is_bounded_by_the_servers_own_ceiling() {
    let router = emulator();
    // One more distribution than the ceiling, so a request asking for far more than the
    // ceiling still has to truncate.
    let total = usize::try_from(MAX_ITEMS).expect("a positive ceiling") + 1;
    for index in 0..total {
        let row = config("ceiling", &format!("ref-ceiling-{index:04}"));
        let answer = create_distribution(&router, &row.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case ceiling-{index}: expected HTTP 201, got {} with {}",
            answer.status, answer.raw
        );
    }

    for asked in [MAX_ITEMS + 1, 1_000, i32::MAX] {
        let answer = list_distributions(&router, &format!("?MaxItems={asked}")).await;
        assert_eq!(
            answer.status, 200,
            "case MaxItems={asked}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        let list = answer.root();
        let items = list
            .child("Items")
            .map(|items| items.children_named("DistributionSummary").len())
            .unwrap_or_default();
        assert_eq!(
            i32::try_from(items).unwrap_or(i32::MAX),
            MAX_ITEMS,
            "case MaxItems={asked}: expected a page bounded at {MAX_ITEMS}, got {items}"
        );
        assert_eq!(
            list.text_at("Quantity"),
            Some(items.to_string().as_str()),
            "case MaxItems={asked}: expected Quantity to equal the items served, got {:?}",
            list.text_at("Quantity")
        );
        // "The value you provided for the MaxItems request parameter."
        assert_eq!(
            list.text_at("MaxItems"),
            Some(asked.to_string().as_str()),
            "case MaxItems={asked}: expected MaxItems to echo the request, got {:?}",
            list.text_at("MaxItems")
        );
        assert_eq!(
            list.text_at("IsTruncated"),
            Some("true"),
            "case MaxItems={asked}: the store holds more than one page, so the page must be \
             truncated, got {:?}",
            list.text_at("IsTruncated")
        );
        let next_marker = list.text_at("NextMarker").unwrap_or_default().to_string();
        assert!(
            is_distribution_id(&next_marker),
            "case MaxItems={asked}: expected a NextMarker naming a distribution, got \
             {next_marker:?}"
        );

        // And the rest is reachable from that marker.
        let rest = list_distributions(&router, &format!("?Marker={next_marker}")).await;
        let rest_items = rest
            .root()
            .child("Items")
            .map(|items| items.children_named("DistributionSummary").len())
            .unwrap_or_default();
        assert_eq!(
            rest_items,
            total - items,
            "case MaxItems={asked}: expected the remaining {} distributions on the next page, \
             got {rest_items}",
            total - items
        );
    }
}

/// A delete needs the current version AND a stored configuration that has the
/// distribution disabled and deployed; after it, the distribution is gone from every
/// read.
#[tokio::test]
async fn delete_requires_a_disabled_distribution_at_its_current_version() {
    let router = emulator();
    let row = config("delete", "ref-delete");
    let created = create_distribution(&router, &row.xml()).await;
    let id = created
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();
    let enabled_etag = created.header("etag").to_string();

    // An enabled distribution is not deletable, even at its current version.
    let enabled = delete_distribution(&router, &id, Some(&enabled_etag)).await;
    assert_eq!(
        enabled.status, 409,
        "case enabled: expected HTTP 409, got {} with {}",
        enabled.status, enabled.raw
    );
    assert_eq!(
        enabled.error_code(),
        "DistributionNotDisabled",
        "case enabled: expected DistributionNotDisabled, got {:?}",
        enabled.error_code()
    );

    // A version that is not current fails the precondition; a value that is no version
    // at all is an invalid If-Match version.
    for (label, if_match, status, code) in [
        ("missing-if-match", None, 400, "InvalidIfMatchVersion"),
        (
            "stale-if-match",
            Some("EZZZZZZZZZZZZZ"),
            412,
            "PreconditionFailed",
        ),
    ] {
        let answer = delete_distribution(&router, &id, if_match).await;
        assert_eq!(
            answer.status, status,
            "case {label}: expected HTTP {status}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {label}: expected {code}, got {:?}",
            answer.error_code()
        );
    }

    // Disable it, then delete it at the version the update handed back.
    let disabled = Config {
        enabled: false,
        ..row.clone()
    };
    let updated =
        update_distribution(&router, &id, &disabled.update_xml(), Some(&enabled_etag)).await;
    assert_eq!(
        updated.status, 200,
        "case disable: expected HTTP 200, got {} with {}",
        updated.status, updated.raw
    );
    let disabled_etag = updated.header("etag").to_string();

    // The disable has not reached the edge locations yet, so the delete is still
    // refused: CloudFront's teardown waits for the disabled configuration to deploy.
    let propagating = delete_distribution(&router, &id, Some(&disabled_etag)).await;
    assert_eq!(
        propagating.status, 409,
        "case disable-propagating: expected HTTP 409, got {} with {}",
        propagating.status, propagating.raw
    );
    assert_eq!(
        propagating.error_code(),
        "DistributionNotDisabled",
        "case disable-propagating: expected DistributionNotDisabled, got {:?}",
        propagating.error_code()
    );

    // Once the clock owes the disabled configuration its deployment, the delete lands.
    tokio::time::sleep(PROPAGATION_DELAY + Duration::from_millis(250)).await;

    let deleted = delete_distribution(&router, &id, Some(&disabled_etag)).await;
    assert_eq!(
        deleted.status, 204,
        "case delete: expected HTTP 204, got {} with {}",
        deleted.status, deleted.raw
    );
    assert!(
        deleted.raw.trim().is_empty(),
        "case delete: expected no body, got {}",
        deleted.raw
    );

    // Gone from every read.
    let fetched = get_distribution(&router, &id).await;
    assert_eq!(
        fetched.status, 404,
        "case after-delete: expected HTTP 404, got {} with {}",
        fetched.status, fetched.raw
    );
    assert_eq!(
        fetched.error_code(),
        "NoSuchDistribution",
        "case after-delete: expected NoSuchDistribution, got {:?}",
        fetched.error_code()
    );
    let listed = list_distributions(&router, "").await;
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some("0"),
        "case after-delete: expected an empty list, got {}",
        listed.raw
    );
    let again = delete_distribution(&router, &id, Some(&disabled_etag)).await;
    assert_eq!(
        again.status, 404,
        "case delete-twice: expected HTTP 404, got {} with {}",
        again.status, again.raw
    );
    assert_eq!(
        again.error_code(),
        "NoSuchDistribution",
        "case delete-twice: expected NoSuchDistribution, got {:?}",
        again.error_code()
    );
}

/// An invalidation is stored under its distribution and read back from there: the batch
/// comes back as submitted, a repeated caller reference is the same request again, and
/// the distribution counts the batches still in progress.
#[tokio::test]
async fn invalidations_are_stored_under_their_distribution() {
    let router = emulator();
    let row = config("invalidation", "ref-invalidation");
    let created = create_distribution(&router, &row.xml()).await;
    let id = created
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();

    // Rows differing in how many paths a batch names, and in their form.
    let batches: &[(&str, &[&str])] = &[
        ("everything", &["/*"]),
        ("one-object", &["/index.html"]),
        (
            "three-objects",
            &["/images/a.png", "/images/b.png", "/css/site.css"],
        ),
    ];

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (index, (case, paths)) in batches.iter().enumerate() {
        let reference = format!("inv-{case}");
        let answer = create_invalidation(&router, &id, &batch_xml(&reference, paths)).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            answer.status, answer.raw
        );
        let invalidation = answer.root();
        assert_eq!(
            invalidation.name, "Invalidation",
            "case {case}: expected an <Invalidation> payload, got <{}>",
            invalidation.name
        );
        let invalidation_id = invalidation
            .text_at("Id")
            .unwrap_or_default()
            .to_string();
        assert!(
            is_invalidation_id(&invalidation_id),
            "case {case}: expected an `I`+13 invalidation id, got {invalidation_id:?}"
        );
        assert!(
            seen.insert(invalidation_id.clone()),
            "case {case}: invalidation id {invalidation_id} was already allocated"
        );
        assert_eq!(
            invalidation.text_at("Status"),
            Some("InProgress"),
            "case {case}: expected a just-created invalidation to be InProgress, got {:?}",
            invalidation.text_at("Status")
        );
        assert!(
            invalidation
                .text_at("CreateTime")
                .is_some_and(|value| !value.is_empty()),
            "case {case}: expected a CreateTime, got {:?}",
            invalidation.text_at("CreateTime")
        );
        assert_eq!(
            answer.header("location"),
            format!("https://{HOST}/{API_VERSION}/distribution/{id}/invalidation/{invalidation_id}"),
            "case {case}: expected the Location of the new invalidation, got {:?}",
            answer.header("location")
        );
        assert_batch(invalidation, &reference, paths, case, "CreateInvalidation");

        // The same record, read back from the store.
        let fetched = get_invalidation(&router, &id, &invalidation_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected HTTP 200 from GetInvalidation, got {} with {}",
            fetched.status, fetched.raw
        );
        assert_eq!(
            fetched.root().text_at("Id"),
            Some(invalidation_id.as_str()),
            "case {case}: expected GetInvalidation to report the same id, got {:?}",
            fetched.root().text_at("Id")
        );
        assert_batch(fetched.root(), &reference, paths, case, "GetInvalidation");

        // The distribution counts the batches still in progress: never more than the
        // batches created so far, and the exact count is pinned below on a distribution
        // whose invalidations are all created in one burst (a batch created earlier in
        // THIS loop may legitimately have settled already).
        let distribution = get_distribution(&router, &id).await;
        let in_progress: usize = distribution
            .root()
            .text_at("InProgressInvalidationBatches")
            .unwrap_or_default()
            .parse()
            .unwrap_or_else(|error| {
                panic!("case {case}: InProgressInvalidationBatches is not a number: {error}")
            });
        assert!(
            in_progress <= index + 1,
            "case {case}: expected at most {} batches in progress, got {in_progress}",
            index + 1
        );

        // "If you make a second invalidation request with the same value for
        // CallerReference, and if the rest of the request is the same, CloudFront
        // doesn't create a new invalidation request."
        let repeated = create_invalidation(&router, &id, &batch_xml(&reference, paths)).await;
        assert_eq!(
            repeated.status, 201,
            "case {case}/repeat: expected HTTP 201, got {} with {}",
            repeated.status, repeated.raw
        );
        assert_eq!(
            repeated.root().text_at("Id"),
            Some(invalidation_id.as_str()),
            "case {case}/repeat: a repeated CallerReference must report the invalidation it \
             already created, got {:?}",
            repeated.root().text_at("Id")
        );

        // The same reference naming different paths is a different request, and the
        // reference is taken.
        let conflicting =
            create_invalidation(&router, &id, &batch_xml(&reference, &["/something-else"])).await;
        assert_eq!(
            conflicting.status, 400,
            "case {case}/conflict: expected HTTP 400, got {} with {}",
            conflicting.status, conflicting.raw
        );
        assert_eq!(
            conflicting.error_code(),
            "InvalidArgument",
            "case {case}/conflict: expected InvalidArgument, got {:?}",
            conflicting.error_code()
        );
    }

    // A batch whose Paths.Quantity disagrees with its Items is refused.
    let inconsistent = create_invalidation(
        &router,
        &id,
        &batch_xml_with_quantity("inv-inconsistent", &["/a", "/b"], 5),
    )
    .await;
    assert_eq!(
        inconsistent.status, 400,
        "case inconsistent-paths: expected HTTP 400, got {} with {}",
        inconsistent.status, inconsistent.raw
    );
    assert_eq!(
        inconsistent.error_code(),
        "InconsistentQuantities",
        "case inconsistent-paths: expected InconsistentQuantities, got {:?}",
        inconsistent.error_code()
    );

    // No invalidation exists outside a distribution, and none of these is a record.
    let unknown_distribution =
        create_invalidation(&router, "E00000000000X", &batch_xml("inv-orphan", &["/*"])).await;
    assert_eq!(
        unknown_distribution.status, 404,
        "case unknown-distribution: expected HTTP 404, got {} with {}",
        unknown_distribution.status, unknown_distribution.raw
    );
    assert_eq!(
        unknown_distribution.error_code(),
        "NoSuchDistribution",
        "case unknown-distribution: expected NoSuchDistribution, got {:?}",
        unknown_distribution.error_code()
    );
    let unknown_invalidation = get_invalidation(&router, &id, "I00000000000X").await;
    assert_eq!(
        unknown_invalidation.status, 404,
        "case unknown-invalidation: expected HTTP 404, got {} with {}",
        unknown_invalidation.status, unknown_invalidation.raw
    );
    assert_eq!(
        unknown_invalidation.error_code(),
        "NoSuchInvalidation",
        "case unknown-invalidation: expected NoSuchInvalidation, got {:?}",
        unknown_invalidation.error_code()
    );
}

/// A batch names at most the quota's worth of paths, and the distribution counts exactly
/// the batches created under it while they are all still in progress.
#[tokio::test]
async fn an_invalidation_batch_is_bounded_and_counted() {
    let router = emulator();
    let row = config("batch-bounds", "ref-batch-bounds");
    let created = create_distribution(&router, &row.xml()).await;
    let id = created
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();

    // A batch at the quota is accepted; one path more is too large.
    let at_limit: Vec<String> = (0..MAX_INVALIDATION_PATHS)
        .map(|index| format!("/object-{index}"))
        .collect();
    let at_limit_refs: Vec<&str> = at_limit.iter().map(String::as_str).collect();
    let accepted = create_invalidation(&router, &id, &batch_xml("inv-at-limit", &at_limit_refs)).await;
    assert_eq!(
        accepted.status, 201,
        "case at-the-quota: expected HTTP 201 for {MAX_INVALIDATION_PATHS} paths, got {} with {}",
        accepted.status,
        accepted.raw.chars().take(400).collect::<String>()
    );
    assert_eq!(
        accepted.root().text_at("InvalidationBatch/Paths/Quantity"),
        Some(MAX_INVALIDATION_PATHS.to_string().as_str()),
        "case at-the-quota: expected the stored batch to carry {MAX_INVALIDATION_PATHS} paths, \
         got {:?}",
        accepted.root().text_at("InvalidationBatch/Paths/Quantity")
    );

    let mut over_limit = at_limit.clone();
    over_limit.push("/one-too-many".to_string());
    let over_limit_refs: Vec<&str> = over_limit.iter().map(String::as_str).collect();
    let refused =
        create_invalidation(&router, &id, &batch_xml("inv-over-limit", &over_limit_refs)).await;
    assert_eq!(
        refused.status, 413,
        "case over-the-quota: expected HTTP 413, got {} with {}",
        refused.status,
        refused.raw.chars().take(400).collect::<String>()
    );
    assert_eq!(
        refused.error_code(),
        "BatchTooLarge",
        "case over-the-quota: expected BatchTooLarge, got {:?}",
        refused.error_code()
    );

    // Two batches created in one burst, then read in the same burst: the count is
    // exactly the two, and the refused one was never stored.
    let fresh = create_distribution(&router, &config("burst", "ref-burst").xml()).await;
    let fresh_id = fresh.root().text_at("Id").unwrap_or_default().to_string();
    for index in 0..2 {
        let answer = create_invalidation(
            &router,
            &fresh_id,
            &batch_xml(&format!("inv-burst-{index}"), &["/*"]),
        )
        .await;
        assert_eq!(
            answer.status, 201,
            "case burst-{index}: expected HTTP 201, got {} with {}",
            answer.status, answer.raw
        );
    }
    let distribution = get_distribution(&router, &fresh_id).await;
    assert_eq!(
        distribution
            .root()
            .text_at("InProgressInvalidationBatches"),
        Some("2"),
        "case burst: expected both batches in progress, got {:?}",
        distribution
            .root()
            .text_at("InProgressInvalidationBatches")
    );
}

/// Assert that `invalidation` carries the batch a row submitted.
fn assert_batch(invalidation: &Node, reference: &str, paths: &[&str], case: &str, context: &str) {
    let batch = invalidation
        .child("InvalidationBatch")
        .unwrap_or_else(|| panic!("case {case} ({context}): no InvalidationBatch"));
    assert_eq!(
        batch.text_at("CallerReference"),
        Some(reference),
        "case {case} ({context}): expected CallerReference {reference:?}, got {:?}",
        batch.text_at("CallerReference")
    );
    assert_eq!(
        batch.text_at("Paths/Quantity"),
        Some(paths.len().to_string().as_str()),
        "case {case} ({context}): expected Paths.Quantity {}, got {:?}",
        paths.len(),
        batch.text_at("Paths/Quantity")
    );
    let stored: Vec<String> = batch
        .at("Paths/Items")
        .map(|items| {
            items
                .children_named("Path")
                .into_iter()
                .map(|path| path.text.clone())
                .collect()
        })
        .unwrap_or_default();
    let expected: Vec<String> = paths.iter().map(|path| (*path).to_string()).collect();
    assert_eq!(
        stored, expected,
        "case {case} ({context}): expected paths {expected:?}, got {stored:?}"
    );
}

/// Every CloudFront list travels in a wrapper that states its own length, and a request
/// whose `Quantity` disagrees with its `Items` is refused — wherever the wrapper sits.
#[tokio::test]
async fn a_wrapper_whose_quantity_disagrees_with_its_items_is_refused() {
    let router = emulator();

    // Each row is a `DistributionConfig` body carrying ONE wrapper whose stated
    // `Quantity` disagrees with the number of items beside it.
    let rows: &[(&str, &str)] = &[
        (
            "origins",
            "<Origins><Quantity>2</Quantity><Items>\
             <Origin><Id>primary</Id><DomainName>a.example.com</DomainName></Origin>\
             </Items></Origins>\
             <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
             <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy></DefaultCacheBehavior>",
        ),
        (
            "aliases",
            "<Aliases><Quantity>0</Quantity><Items><CNAME>cdn.example.com</CNAME></Items></Aliases>\
             <Origins><Quantity>1</Quantity><Items>\
             <Origin><Id>primary</Id><DomainName>a.example.com</DomainName></Origin>\
             </Items></Origins>\
             <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
             <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy></DefaultCacheBehavior>",
        ),
        (
            "default-cache-behavior-allowed-methods",
            "<Origins><Quantity>1</Quantity><Items>\
             <Origin><Id>primary</Id><DomainName>a.example.com</DomainName></Origin>\
             </Items></Origins>\
             <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
             <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>\
             <AllowedMethods><Quantity>3</Quantity><Items>\
             <Method>GET</Method><Method>HEAD</Method></Items></AllowedMethods>\
             </DefaultCacheBehavior>",
        ),
        (
            "custom-error-responses",
            "<Origins><Quantity>1</Quantity><Items>\
             <Origin><Id>primary</Id><DomainName>a.example.com</DomainName></Origin>\
             </Items></Origins>\
             <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
             <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy></DefaultCacheBehavior>\
             <CustomErrorResponses><Quantity>1</Quantity></CustomErrorResponses>",
        ),
        (
            "geo-restriction",
            "<Origins><Quantity>1</Quantity><Items>\
             <Origin><Id>primary</Id><DomainName>a.example.com</DomainName></Origin>\
             </Items></Origins>\
             <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
             <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy></DefaultCacheBehavior>\
             <Restrictions><GeoRestriction><RestrictionType>whitelist</RestrictionType>\
             <Quantity>2</Quantity><Items><Location>DE</Location></Items>\
             </GeoRestriction></Restrictions>",
        ),
    ];

    for (case, middle) in rows {
        let body = format!(
            "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <CallerReference>ref-{case}</CallerReference>{middle}\
             <Comment>inconsistent</Comment><Enabled>true</Enabled></DistributionConfig>"
        );
        let answer = create_distribution(&router, &body).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InconsistentQuantities",
            "case {case}: expected InconsistentQuantities, got {:?}",
            answer.error_code()
        );
    }

    // The same wrappers, each stating the length it really carries, are accepted — so
    // the refusals above are about the disagreement and nothing else.
    let consistent = format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>ref-consistent</CallerReference>\
         <Aliases><Quantity>1</Quantity><Items><CNAME>cdn.example.com</CNAME></Items></Aliases>\
         <Origins><Quantity>1</Quantity><Items>\
         <Origin><Id>primary</Id><DomainName>a.example.com</DomainName></Origin>\
         </Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>\
         <AllowedMethods><Quantity>2</Quantity><Items>\
         <Method>GET</Method><Method>HEAD</Method></Items></AllowedMethods>\
         </DefaultCacheBehavior>\
         <Comment>consistent</Comment>\
         {CUSTOM_VIEWER_CERTIFICATE}\
         <CustomErrorResponses><Quantity>0</Quantity></CustomErrorResponses>\
         <Restrictions><GeoRestriction><RestrictionType>whitelist</RestrictionType>\
         <Quantity>1</Quantity><Items><Location>DE</Location></Items>\
         </GeoRestriction></Restrictions>\
         <Enabled>true</Enabled></DistributionConfig>"
    );
    let accepted = create_distribution(&router, &consistent).await;
    assert_eq!(
        accepted.status, 201,
        "case every-wrapper-consistent: expected HTTP 201, got {} with {}",
        accepted.status, accepted.raw
    );

    // And an update is held to the same rule.
    let id = accepted
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();
    let etag = accepted.header("etag").to_string();
    let inconsistent = consistent.replace(
        "<Aliases><Quantity>1</Quantity>",
        "<Aliases><Quantity>7</Quantity>",
    );
    let refused = update_distribution(&router, &id, &inconsistent, Some(&etag)).await;
    assert_eq!(
        refused.status, 400,
        "case update-inconsistent: expected HTTP 400, got {} with {}",
        refused.status, refused.raw
    );
    assert_eq!(
        refused.error_code(),
        "InconsistentQuantities",
        "case update-inconsistent: expected InconsistentQuantities, got {:?}",
        refused.error_code()
    );
}

/// The clock, not a later call, owes a distribution its deployment and an invalidation
/// its completion: both are `InProgress` straight after the call that starts them, and
/// terminal once the span has passed — and the distribution's in-progress batch count
/// falls with them.
#[tokio::test]
async fn the_clock_settles_propagation_and_invalidation() {
    let router = emulator();
    let row = config("clock", "ref-clock");
    let created = create_distribution(&router, &row.xml()).await;
    let id = created
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();

    let invalidation =
        create_invalidation(&router, &id, &batch_xml("inv-clock", &["/*"])).await;
    let invalidation_id = invalidation
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();

    // Straight after the calls, both are still in progress: the span is never zero.
    let immediate = get_distribution(&router, &id).await;
    assert_eq!(
        immediate.root().text_at("Status"),
        Some("InProgress"),
        "case immediate: expected InProgress, got {:?}",
        immediate.root().text_at("Status")
    );
    assert_eq!(
        immediate.root().text_at("InProgressInvalidationBatches"),
        Some("1"),
        "case immediate: expected one batch in progress, got {:?}",
        immediate.root().text_at("InProgressInvalidationBatches")
    );

    // Once the span has passed, the clock has settled both — without any call having
    // asked for it.
    tokio::time::sleep(PROPAGATION_DELAY + Duration::from_millis(250)).await;

    let settled = get_distribution(&router, &id).await;
    assert_eq!(
        settled.root().text_at("Status"),
        Some("Deployed"),
        "case settled: expected Deployed, got {:?}",
        settled.root().text_at("Status")
    );
    assert_eq!(
        settled.root().text_at("InProgressInvalidationBatches"),
        Some("0"),
        "case settled: expected no batch in progress, got {:?}",
        settled.root().text_at("InProgressInvalidationBatches")
    );
    let settled_invalidation = get_invalidation(&router, &id, &invalidation_id).await;
    assert_eq!(
        settled_invalidation.root().text_at("Status"),
        Some("Completed"),
        "case settled: expected Completed, got {:?}",
        settled_invalidation.root().text_at("Status")
    );

    // A read does not move anything else: the version and the configuration are what
    // the create left behind.
    assert_eq!(
        settled.header("etag"),
        created.header("etag"),
        "case settled: a read must not change the distribution's version"
    );
    row.assert_matches(
        settled
            .root()
            .child("DistributionConfig")
            .unwrap_or_else(|| panic!("case settled: no DistributionConfig")),
        "GetDistribution after the clock settled",
    );

    // And an update starts the span again.
    let changed = Config {
        comment: "changed after deployment".to_string(),
        ..row.clone()
    };
    let updated =
        update_distribution(&router, &id, &changed.update_xml(), Some(settled.header("etag")))
            .await;
    assert_eq!(
        updated.root().text_at("Status"),
        Some("InProgress"),
        "case re-deploy: expected an updated distribution to be InProgress again, got {:?}",
        updated.root().text_at("Status")
    );
}
