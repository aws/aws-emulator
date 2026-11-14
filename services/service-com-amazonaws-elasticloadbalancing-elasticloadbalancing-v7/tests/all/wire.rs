// SPDX-License-Identifier: CC0-1.0

//! The wire harness this crate's suites share: the emulator under test, one awsQuery
//! call, and a reader for the XML envelope it answers with.
//!
//! Every assertion in this crate's suites goes through the awsQuery wire protocol on the
//! multi-service router — the same path `core/router` serves — so what is checked is the
//! emulator's HTTP behaviour, not a handler called directly.
//!
//! What a health check of a registered instance observes is a fact Amazon EC2 owns, so the
//! harness also assembles the two services on ONE engine ([`emulator_with_ec2`]), sends an
//! ec2Query call ([`ec2_call`]), launches a backend through EC2's own API
//! ([`launch_instance`]), moves one through EC2's own API and waits out the move EC2 owes it
//! ([`move_instance`], [`settle_ec2_transition`]) and places a registration's stored instant
//! in the past ([`age_registration`], and [`age_registrations`] for every registration of
//! one load balancer) — once here, because every suite that drives an instance's health
//! needs the same ones.
//!
//! Declared in `tests/all.rs` alongside the suites that use it, so there is ONE request
//! builder and ONE envelope reader rather than a copy per suite.

use bytes::Bytes;
use service_com_amazonaws_ec2_amazonec2 as ec2_service;
use service_com_amazonaws_ec2_amazonec2::state::TRANSITION_DELAY;
use service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::state::ComAmazonawsElasticloadbalancingElasticloadbalancingV7State as ElbState;
use service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::Duration;

/// The service's model version, as every awsQuery request carries it.
pub(crate) const API_VERSION: &str = "2012-06-01";

/// The API version an ec2Query body names, as an EC2 client sends it.
pub(crate) const EC2_API_VERSION: &str = "2016-11-15";

/// The emulator under test: ONE shared engine with this service mounted on the
/// multi-service router, exactly as the production router assembles it.
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// The same emulator, plus the shared engine it was mounted on — the handle a suite
/// needs to read this service's stored state, or to place a stored instant in the past so
/// a transition the clock owes is due without waiting for it.
pub(crate) fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router
        .set_request_authentication(false)
        .register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// The emulator with Amazon EC2 mounted BESIDE this service on the same engine, which is
/// how the production router assembles them: a load balancer's health checks read the EC2
/// inventory of that one engine, so a suite that drives an instance's health needs both
/// services behind one front door.
pub(crate) fn emulator_with_ec2() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router
        .set_request_authentication(false)
        .register_service(EmulatedService::new(Arc::clone(&engine)))
        .register_service(ec2_service::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// POST one ec2Query action, signed for `region` and for the `ec2` service so the router
/// hands it to the EC2 emulator.
pub(crate) async fn ec2_call(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[(String, String)],
) -> Wire {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (key, value) in params {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/ec2/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    Wire {
        status: response.status().as_u16(),
        content_type: response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string(),
        body: String::from_utf8(response.body().to_vec()).expect("a UTF-8 XML body"),
    }
}

/// The AMI a launch below names. Any well-formed image id: what a health check turns on is
/// the state EC2 reports for the instance, not the image it came from.
pub(crate) const AMI: &str = "ami-0123456789abcdef0";

/// Launch ONE instance in `region` through EC2's OWN API and answer the id EC2 minted for
/// it, so a backend a suite registers is a real EC2 resource rather than an id made up here.
pub(crate) async fn launch_instance(
    router: &MultiServiceRouter,
    region: &str,
    label: &str,
) -> String {
    launch_instance_in_zone(router, region, None, label).await
}

/// [`launch_instance`], into the Availability Zone `zone` names — the placement a launch
/// asks for through `Placement.AvailabilityZone`.
///
/// Which zone an instance is in is EC2's to say, and it is what decides whether a load
/// balancer node may send it a request while cross-zone load balancing is off, so a suite
/// that drives that attribute launches its backends into zones of its own choosing.
pub(crate) async fn launch_instance_in_zone(
    router: &MultiServiceRouter,
    region: &str,
    zone: Option<&str>,
    label: &str,
) -> String {
    let mut params = vec![
        param("ImageId", AMI),
        param("MinCount", "1"),
        param("MaxCount", "1"),
    ];
    if let Some(zone) = zone {
        params.push(param("Placement.AvailabilityZone", zone));
    }
    let launched = ec2_call(router, region, "RunInstances", &params).await;
    assert_eq!(
        launched.status, 200,
        "case {label:?}: RunInstances must answer 200, got {}",
        launched.body
    );
    find(parse(&launched.body).root_element(), "instanceId")
        .and_then(Element::text)
        .unwrap_or_else(|| panic!("case {label:?}: RunInstances reported no instance"))
        .to_string()
}

/// Move the stored registration of `instance_id` with `load_balancer_name` `back` into the
/// past, so the health checks the clock owes it are due without a suite waiting for them.
///
/// The registrations beside it are left where they are, which is how one answer comes to
/// carry registrations at different points of their timelines.
pub(crate) fn age_registration(
    engine: &engine::InMemoryEngine,
    region: &str,
    load_balancer_name: &str,
    instance_id: &str,
    back: Duration,
) {
    let state = engine.state::<ElbState>();
    let mut guard = state.lock();
    let record = guard
        .load_balancers_in_mut(region)
        .and_then(|records| records.get_mut(load_balancer_name))
        .unwrap_or_else(|| panic!("case {load_balancer_name:?}: no stored load balancer"));
    let at = record
        .instances
        .registered_at(instance_id)
        .unwrap_or_else(|| {
            panic!("case {load_balancer_name:?}: {instance_id} carries no registration")
        });
    let moved = at.checked_sub(back).unwrap_or_else(|| {
        panic!("case {load_balancer_name:?}: {instance_id} cannot be aged by {back:?}")
    });
    assert!(
        record.instances.restamp(instance_id, moved),
        "case {load_balancer_name:?}: {instance_id} must still be registered"
    );
}

/// [`age_registration`] for EVERY instance registered with one load balancer.
pub(crate) fn age_registrations(
    engine: &engine::InMemoryEngine,
    region: &str,
    load_balancer_name: &str,
    back: Duration,
) {
    let registered: Vec<String> = {
        let state = engine.state::<ElbState>();
        let guard = state.lock();
        guard
            .load_balancers_in(region)
            .and_then(|records| records.get(load_balancer_name))
            .unwrap_or_else(|| panic!("case {load_balancer_name:?}: no stored load balancer"))
            .instances
            .iter()
            .cloned()
            .collect()
    };
    for instance_id in registered {
        age_registration(engine, region, load_balancer_name, &instance_id, back);
    }
}

/// Drive one EC2 instance-state action over `instance_id` through EC2's OWN API and let the
/// move EC2 owes it fall due, so the state EC2 reports afterwards is the terminal one the
/// action asked for.
pub(crate) async fn move_instance(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    action: &str,
    instance_id: &str,
) {
    let moved = ec2_call(router, region, action, &[param("InstanceId.1", instance_id)]).await;
    assert_eq!(
        moved.status, 200,
        "case {case:?}: {action} must answer 200, got {}",
        moved.body
    );
    settle_ec2_transition().await;
}

/// Wait until a state change EC2 started before now has certainly fallen due. Derived from
/// EC2's own span, not a literal, so the two cannot drift.
pub(crate) async fn settle_ec2_transition() {
    ::tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// One wire answer: the HTTP status, the `Content-Type` it was answered with, and the XML
/// body.
pub(crate) struct Wire {
    pub(crate) status: u16,
    /// The answer's `Content-Type` header, or an empty string when it carries none. An
    /// awsQuery answer — success or refusal — is XML, so a suite that asserts a refusal is
    /// readable by an awsQuery client asserts this too.
    pub(crate) content_type: String,
    pub(crate) body: String,
}

impl Wire {
    /// The `<Code>` of an `<ErrorResponse>` envelope.
    pub(crate) fn error_code(&self) -> String {
        let doc = parse(&self.body);
        find(doc.root_element(), "Code")
            .and_then(Element::text)
            .unwrap_or_default()
            .to_string()
    }

    /// The `<{Operation}Result>` element of a success envelope.
    pub(crate) fn result<'a>(&self, doc: &'a Element) -> &'a Element {
        assert_eq!(
            self.status, 200,
            "expected a success envelope, got {}",
            self.body
        );
        doc.root_element()
            .children()
            .find(|node| node.name.ends_with("Result"))
            .unwrap_or_else(|| panic!("no …Result element in {}", self.body))
    }
}

/// Percent-encode one `application/x-www-form-urlencoded` component: every byte outside
/// RFC 3986's unreserved set (`A-Za-z0-9` and `-`, `_`, `.`, `~`) becomes `%XX`.
pub(crate) fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(*byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// POST one awsQuery action, signed for `region` so the service resolves that region
/// from the request's SigV4 credential scope.
///
/// The envelope names this model's own API version; a suite that drives the shared
/// endpoint's OTHER version uses [`call_at_version`].
pub(crate) async fn call(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[(String, String)],
) -> Wire {
    call_at_version(router, region, action, Some(API_VERSION), params).await
}

/// POST one awsQuery action at the API version its envelope names — or with no `Version`
/// field at all when `version` is `None`.
///
/// The version is the other half of an awsQuery routing key, and this endpoint serves two
/// API versions out of two models (`src/service/api_version.rs`), so a suite that pins
/// which model answers varies it. [`call`] is this call at this model's own version, which
/// is what every other suite sends.
pub(crate) async fn call_at_version(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    version: Option<&str>,
    params: &[(String, String)],
) -> Wire {
    let mut body = format!("Action={}", form_encode(action));
    if let Some(version) = version {
        body.push_str(&format!("&Version={}", form_encode(version)));
    }
    for (key, value) in params {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/elasticloadbalancing/\
                 aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    Wire {
        status: response.status().as_u16(),
        content_type: response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string(),
        body: String::from_utf8(response.body().to_vec()).expect("a UTF-8 XML body"),
    }
}

/// One form field.
pub(crate) fn param(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// One element of a parsed response envelope: its local name, its own character data and
/// its child elements, which is everything these suites assert on.
///
/// The reader below is deliberately tiny and test-only. awsQuery response envelopes are
/// flat element trees with one default `xmlns` and no attributes, comments, CDATA or
/// namespace prefixes, so reading them needs no XML library — and no crates.io XML parser
/// is pulled in for test-only use of it. It is NOT a general XML parser and nothing
/// outside `tests/` uses it.
#[derive(Debug, Default)]
pub(crate) struct Element {
    pub(crate) name: String,
    text: String,
    kids: Vec<Element>,
}

impl Element {
    /// The document's root element, so a caller reads like the envelope it holds.
    pub(crate) fn root_element(&self) -> &Element {
        self
    }

    /// This element's own character data, or `None` when it carries none.
    pub(crate) fn text(&self) -> Option<&str> {
        Some(self.text.as_str()).filter(|text| !text.is_empty())
    }

    pub(crate) fn children(&self) -> impl Iterator<Item = &Element> {
        self.kids.iter()
    }

    /// This element and every element below it, outermost first.
    fn descendants<'a>(&'a self, into: &mut Vec<&'a Element>) {
        into.push(self);
        for kid in &self.kids {
            kid.descendants(into);
        }
    }
}

/// Read a response envelope. Panics with the body when it is not the well-formed element
/// tree the emulator is supposed to answer with.
pub(crate) fn parse(xml: &str) -> Element {
    let mut at = 0usize;
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;

    while at < xml.len() {
        let Some(open) = xml[at..].find('<').map(|offset| at + offset) else {
            break;
        };
        // Character data between two tags belongs to the element that encloses it.
        if open > at {
            if let Some(current) = stack.last_mut() {
                current.text.push_str(&unescape(&xml[at..open]));
            }
        }
        let Some(close) = xml[open..].find('>').map(|offset| open + offset) else {
            panic!("unterminated tag in {xml}");
        };
        let tag = &xml[open + 1..close];
        at = close + 1;

        // `<?xml ...?>` declarations and comments carry nothing these suites read.
        if tag.starts_with('?') || tag.starts_with('!') {
            continue;
        }
        if let Some(name) = tag.strip_prefix('/') {
            let Some(mut finished) = stack.pop() else {
                panic!("closing tag </{name}> with no open element in {xml}");
            };
            assert_eq!(
                finished.name,
                local_name(name),
                "mismatched closing tag in {xml}"
            );
            finished.text = finished.text.trim().to_string();
            match stack.last_mut() {
                Some(parent) => parent.kids.push(finished),
                None => root = Some(finished),
            }
            continue;
        }
        let self_closing = tag.ends_with('/');
        let tag = tag.trim_end_matches('/');
        // Attributes (only the envelope's `xmlns`) are not read: the name ends at the
        // first whitespace.
        let name = local_name(tag.split_whitespace().next().unwrap_or_default());
        let element = Element {
            name,
            ..Element::default()
        };
        if self_closing {
            match stack.last_mut() {
                Some(parent) => parent.kids.push(element),
                None => root = Some(element),
            }
        } else {
            stack.push(element);
        }
    }

    assert!(stack.is_empty(), "unclosed element in {xml}");
    root.unwrap_or_else(|| panic!("no element in {xml}"))
}

/// The local part of an element name: the envelope carries one default `xmlns`, so a
/// prefix is dropped rather than resolved.
fn local_name(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).to_string()
}

/// The five XML predefined entities, the only escapes an awsQuery envelope carries.
fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The first descendant whose local name is `tag` (the envelope carries a default
/// xmlns, so only the local name is compared).
pub(crate) fn find<'a>(node: &'a Element, tag: &str) -> Option<&'a Element> {
    let mut all: Vec<&Element> = Vec::new();
    node.descendants(&mut all);
    all.into_iter().find(|candidate| candidate.name == tag)
}

/// The direct child whose local name is `tag`.
pub(crate) fn kid<'a>(node: &'a Element, tag: &str) -> Option<&'a Element> {
    node.children().find(|candidate| candidate.name == tag)
}

/// The `<Message>` of an answer's `<Error>`, or an empty string.
///
/// Here, once, because every suite that asserts a refusal reads it.
pub(crate) fn error_message(answer: &Wire) -> String {
    let doc = parse(&answer.body);
    find(doc.root_element(), "Message")
        .and_then(Element::text)
        .unwrap_or_default()
        .to_string()
}

/// The `<Message>` of an answer's `<Error>` read from the RAW body, byte for byte.
///
/// [`parse`] trims each element's character data, which is what a suite comparing a
/// sentence wants — and exactly what a suite asserting the message's own TEXT must not do:
/// a refusal whose wording ends in a space is a different message to a client matching on
/// it, and a trimming reader cannot tell the two apart. So this reads between the tags
/// instead, changing nothing about what the serializer wrote.
pub(crate) fn error_message_bytes(answer: &Wire) -> String {
    let (_, after) = answer
        .body
        .split_once("<Message>")
        .unwrap_or_else(|| panic!("no <Message> element in {}", answer.body));
    let (text, _) = after
        .split_once("</Message>")
        .unwrap_or_else(|| panic!("unterminated <Message> in {}", answer.body));
    unescape(text)
}

pub(crate) fn kid_text(node: &Element, tag: &str) -> Option<String> {
    kid(node, tag).and_then(Element::text).map(str::to_string)
}

/// Whether `id` has the 8-4-4-4-12 hexadecimal form every AWS request id carries.
///
/// Here, once, because a suite that reads an id a response MINTS asserts its form rather
/// than a value: the id names one call and nothing else.
pub(crate) fn is_request_id(id: &str) -> bool {
    let mut groups = id.split('-');
    for width in [8usize, 4, 4, 4, 12] {
        let Some(group) = groups.next() else {
            return false;
        };
        if group.len() != width || !group.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return false;
        }
    }
    groups.next().is_none()
}

/// The `<member>` elements of the list child named `tag`.
pub(crate) fn members<'a>(node: &'a Element, tag: &str) -> Vec<&'a Element> {
    kid(node, tag)
        .map(|list| {
            list.children()
                .filter(|candidate| candidate.name == "member")
                .collect()
        })
        .unwrap_or_default()
}

/// The text of each `<member>` of the list child named `tag`.
pub(crate) fn member_texts(node: &Element, tag: &str) -> Vec<String> {
    members(node, tag)
        .into_iter()
        .map(|member| member.text().unwrap_or_default().to_string())
        .collect()
}
