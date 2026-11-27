// SPDX-License-Identifier: CC0-1.0

//! The ELBv2 provisioning loop, end to end over the wire: create a load balancer,
//! give it a target group and a listener, register targets, read everything back and
//! tear it down.
//!
//! Every request goes through the router exactly as a client's would — awsQuery form
//! body in, `<{Operation}Response>` XML out — so these tests exercise the wire
//! protocol as well as the handlers, and every assertion is made on what a later
//! read reports rather than on what the create answered.
//!
//! This module also carries the harness every suite in this crate shares: the router
//! over one engine ([`emulator`]), the awsQuery request helpers, the XML readers, and
//! the seeding of a record a suite needs at an exact instant ([`seed_trust_store`]).

use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::{
    ComAmazonawsElasticloadbalancingv2ElasticloadbalancingV10State as ElbState, TrustStoreRecord,
    LISTENERS_PER_LOAD_BALANCER, TRUST_STORE_CREATING,
};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// The region every request below is signed for; the ARNs and DNS names the service
/// mints belong to it.
pub(crate) const REGION: &str = "us-west-2";

/// The API version every awsQuery request carries, read from the model's own statement
/// of its service version (`src/service/shared_endpoint.rs`) rather than restated beside
/// it, so one definition serves every suite in this crate.
pub(crate) const VERSION: &str =
    service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::service::shared_endpoint::API_VERSION;

/// The canonical account id every emulator ARN carries.
pub(crate) fn account() -> &'static str {
    emulator_server::emulator_account_id()
}

/// A router carrying this service over one shared engine, plus that engine — what the
/// production router mounts, minus the other services, with the store reachable so a
/// suite can seed a record at an exact instant.
pub(crate) fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// One trust store to seed: its name, how many CA certificates its bundle held, how
/// many revoked entries its revocation lists hold, whether the clock still owes it its
/// move to `ACTIVE`, and the tags it carries.
#[derive(Default)]
pub(crate) struct TrustStoreSeed<'a> {
    pub(crate) name: &'a str,
    pub(crate) certificates: i32,
    pub(crate) revoked: i64,
    pub(crate) settled: bool,
    pub(crate) tags: &'a [(String, String)],
}

/// The instant seeded records are ordered from, so creation order is exact rather than
/// dependent on how fast a suite runs.
fn seed_epoch() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// Put one trust store in the store at creation position `index` and answer its ARN.
///
/// A suite seeds rather than creates when what it needs is a record at an exact INSTANT:
/// reads and writes go through `service_..._v10::state::service_state`'s store, exactly as
/// a handler's do, so a seeded record is the same record the handler serves, and placing
/// the instant the `CREATING -> ACTIVE` move is owed at either side of now is what lets
/// that move be asserted without sleeping for it. A suite that drives the whole create
/// instead stages a CA certificates bundle in Amazon S3 and calls `CreateTrustStore`
/// (`all/trust_store_lifecycle.rs`).
///
/// The ARN is minted the way the service mints one — `emulator_arn` over this service's
/// own namespace, with a 16-hex resource id — so a seeded record carries the same
/// identifier a create would give it.
pub(crate) fn seed_trust_store(
    engine: &Arc<engine::InMemoryEngine>,
    index: usize,
    seed: &TrustStoreSeed<'_>,
) -> String {
    let id = format!("{:016x}", 0x5100_0000_0000_0000u64 + index as u64);
    let arn = emulator_server::emulator_arn(
        "elasticloadbalancing",
        REGION,
        &format!("truststore/{}/{id}", seed.name),
    );
    let now = SystemTime::now();
    let record = TrustStoreRecord {
        arn: arn.clone(),
        name: seed.name.to_string(),
        status: TRUST_STORE_CREATING.to_string(),
        // A settled record is one whose move to `ACTIVE` the clock already owed; an
        // unsettled one is still inside its settling span.
        active_at: if seed.settled {
            now - Duration::from_secs(1)
        } else {
            now + Duration::from_secs(3600)
        },
        created_at: seed_epoch() + Duration::from_secs(index as u64),
        number_of_ca_certificates: seed.certificates,
        // The Amazon S3 address those certificates were read from. A seeded record names
        // none: nothing is staged for it, and only the bundle retrieval (its own issue)
        // reads the address back.
        ca_certificates_bundle: None,
        total_revoked_entries: seed.revoked,
        // The revocation files a trust store holds. A seeded record holds none: a file is
        // added through `AddTrustStoreRevocations`, which reads the list out of an object
        // staged in Amazon S3 (`all/trust_store_revocations.rs`).
        revocations: Default::default(),
        tags: seed
            .tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<HashMap<String, String>>(),
        // The policy a SHARE of this trust store would attach. Seeded as none, and set
        // by the one suite that reads a policy back
        // (`all/load_balancer_network_capacity_and_policy.rs`).
        resource_policy: None,
    };
    let state = engine.state::<ElbState>();
    let mut store = state.lock();
    store
        .name_to_trust_store_arn
        .insert(record.name.clone(), arn.clone());
    store.trust_stores.insert(arn.clone(), record);
    arn
}

/// A router carrying only this service, over one shared engine — what the production
/// router mounts, minus the other services.
fn router() -> MultiServiceRouter {
    emulator().0
}

/// One answer from the emulator.
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) body: String,
}

impl Reply {
    /// The text of the first element named `tag`, or `None` when it is absent or
    /// empty.
    pub(crate) fn text(&self, tag: &str) -> Option<String> {
        first_text(&self.body, tag)
    }

    /// The `<Code>` of an awsQuery error envelope.
    pub(crate) fn error_code(&self) -> String {
        self.text("Code").unwrap_or_default()
    }
}

/// Percent-encode one form component, so an ARN or a path crosses the wire intact.
///
/// Hand-written, as in several sibling crates' suites, for one reason: this branch
/// must not change `Cargo.lock`. That file is the only one every other service's
/// landing also edits, and the gate's squash has refused this change on overlapping
/// `Cargo.lock` content six times, so the suite reads and writes the wire bytes with
/// the crates the service already depends on. Escaping every non-alphanumeric byte is
/// always a valid form encoding.
///
/// `pub(crate)` because the other suites in this one test binary
/// (`all/shared_endpoint_routing.rs` and `all/query_error_envelope.rs`) write the same
/// wire bytes and read them with the same helpers below: one encoder and one element
/// reader for the crate, so the suites cannot drift on encoding or on how an empty
/// element is reported.
pub(crate) fn encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => out.push(char::from(*byte)),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// One form field.
pub(crate) fn p(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// The form fields of one awsQuery request.
type Form = Vec<(String, String)>;

/// One row of the health-check default table: the case name, the request fields and
/// what the stored target group must then report.
type HealthCheckCase = (&'static str, Form, HealthCheckExpectation);

/// One row of the refusal table: the case name, the request fields, the expected
/// HTTP status and the expected awsQuery error code (empty when none).
type RefusalCase = (&'static str, Form, u16, &'static str);

/// `Action` plus `Version`, the two fields every awsQuery request carries.
pub(crate) fn action(name: &str) -> Vec<(String, String)> {
    vec![p("Action", name), p("Version", VERSION)]
}

/// Send one awsQuery request: `pairs` are the form fields, `Action` included.
///
/// Signed for [`REGION`], which is the Region every suite's ARNs and DNS names belong to.
/// A suite that asserts what the service mints for ANOTHER Region sends the same request
/// through [`call_in_region`], which is where the request is built: one builder for the
/// crate, so no suite can drift on how an awsQuery request is addressed.
pub(crate) async fn call(router: &MultiServiceRouter, pairs: &[(String, String)]) -> Reply {
    call_in_region(router, REGION, pairs).await
}

/// Send one awsQuery request signed for `region`, which is the Region the service reads off
/// the SigV4 credential scope.
pub(crate) async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    pairs: &[(String, String)],
) -> Reply {
    let body = pairs
        .iter()
        .map(|(key, value)| format!("{}={}", encode(key), encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    // Signed for this model's OWN routing key, which is what addresses it directly: the
    // `elasticloadbalancing` signing name both Elastic Load Balancing models declare is
    // claimed by the Classic front door (see `src/service.rs`'s `ROUTE_KEY`), which
    // serves this model's operations for a `Version=2015-12-01` envelope. This suite
    // mounts this service ALONE, so it addresses it by its own key; that the shared name
    // reaches these same operations is pinned in
    // `core/router/tests/all/elb_classic_and_v2_signing_name_routing.rs`, which needs
    // both services mounted.
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://elasticloadbalancingv2.{region}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/elasticloadbalancingv2/aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    Reply { status, body }
}

/// The text of the first element named `tag`, or `None` when it is absent or carries
/// nothing (an empty list serializes as `<Tag></Tag>`).
pub(crate) fn first_text(xml: &str, tag: &str) -> Option<String> {
    let (inner, _) = element_span(xml, tag, 0)?;
    (!inner.is_empty()).then(|| inner.to_string())
}

/// The XML inside each `<member>` of the first element named `container`.
pub(crate) fn members(xml: &str, container: &str) -> Vec<String> {
    let Some((inner, _)) = element_span(xml, container, 0) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let mut at = 0usize;
    while let Some((member, past)) = element_span(inner, "member", at) {
        found.push(member.to_string());
        at = past;
    }
    found
}

/// The `TargetHealth` block of one target health description — the element the health's own
/// `State`, `Reason` and `Description` are read out of.
///
/// `pub(crate)` because every suite that reads a reported health reads it through this: a
/// description also carries an `AdministrativeOverride` whose members are named `State`,
/// `Reason` and `Description` too, so a read of the health is scoped to the health's own
/// element rather than taking the first element of that name in the description — which is
/// the override's for a `healthy` target, whose health carries neither a reason nor a
/// description ("if the target state is healthy, a reason code is not provided",
/// `TargetHealth$Reason`).
pub(crate) fn health_block(description: &str) -> String {
    first_text(description, "TargetHealth").unwrap_or_default()
}

/// The `Key`/`Value` pairs of every `<member>` of the first `Attributes` list of `body`,
/// IN ORDER.
///
/// `pub(crate)` because the three attribute suites
/// (`all/load_balancer_attributes.rs`, `all/target_group_attribute_writes.rs`,
/// `all/attribute_collections.rs`) all compare an attribute collection as the ordered
/// list it comes back as: ONE reader for the crate, so they cannot drift on how it is
/// read.
pub(crate) fn attribute_sequence(body: &str) -> Vec<(String, String)> {
    members(body, "Attributes")
        .iter()
        .map(|member| {
            (
                first_text(member, "Key").unwrap_or_default(),
                first_text(member, "Value").unwrap_or_default(),
            )
        })
        .collect()
}

/// The XML inside the first element named `name` at or after `from`, and the offset
/// just past that element's closing tag.
///
/// The responses are the framework's own serializer output: elements, text, and the
/// `xmlns` attribute it puts on the response root and on each `<member>` — no
/// comments, CDATA or processing instructions. This walks the opening and closing
/// tags of `name` only, so a nested element of the same name (a member list inside a
/// member) cannot end it early. Every assertion below reads through it, including the
/// element counts, so a mistake here fails the suite rather than hiding.
fn element_span<'a>(xml: &'a str, name: &str, from: usize) -> Option<(&'a str, usize)> {
    let (_, inner_start, self_closing) = find_tag(xml, name, from)?;
    if self_closing {
        return Some(("", inner_start));
    }
    let close = format!("</{name}>");
    let mut depth = 1usize;
    let mut cursor = inner_start;
    while depth > 0 {
        let next_close = xml[cursor..].find(&close).map(|at| cursor + at)?;
        match find_tag(xml, name, cursor) {
            Some((open_at, inner, false)) if open_at < next_close => {
                depth += 1;
                cursor = inner;
            }
            Some((open_at, past, true)) if open_at < next_close => {
                // `<name/>` opens and closes at once.
                cursor = past;
            }
            _ => {
                depth -= 1;
                if depth == 0 {
                    return Some((&xml[inner_start..next_close], next_close + close.len()));
                }
                cursor = next_close + close.len();
            }
        }
    }
    None
}

/// The offsets of the first opening tag named `name` at or after `from`: where the
/// tag starts, where its content starts, and whether it is self-closing.
fn find_tag(xml: &str, name: &str, from: usize) -> Option<(usize, usize, bool)> {
    let needle = format!("<{name}");
    let mut at = from;
    loop {
        let start = xml[at..].find(&needle).map(|found| at + found)?;
        let after = start + needle.len();
        let rest = &xml[after..];
        if rest.starts_with('>') {
            return Some((start, after + 1, false));
        }
        if rest.starts_with("/>") {
            return Some((start, after + 2, true));
        }
        if rest.starts_with(|c: char| c.is_ascii_whitespace()) {
            // Skip this tag's attributes to its own `>`.
            let end = rest.find('>').map(|found| found + after)?;
            return Some((start, end + 1, xml[..end].ends_with('/')));
        }
        // A longer element name that merely starts with `name`.
        at = after;
    }
}

/// Create a load balancer with two subnets and answer its ARN.
async fn create_alb(router: &MultiServiceRouter, name: &str) -> String {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", name));
    form.push(p("Subnets.member.1", "subnet-11111111"));
    form.push(p("Subnets.member.2", "subnet-22222222"));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create {name}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    // A load balancer AWS returns is not ready yet, and the create answers inside the
    // same locked section that stored it, so this is exact rather than racy.
    assert_eq!(
        reply.text("Code").as_deref(),
        Some("provisioning"),
        "case create {name}: a new load balancer starts provisioning, got {}",
        reply.body
    );
    reply
        .text("LoadBalancerArn")
        .unwrap_or_else(|| panic!("case create {name}: no LoadBalancerArn in {}", reply.body))
}

/// Create an HTTP/80 target group and answer its ARN.
pub(crate) async fn create_http_target_group(router: &MultiServiceRouter, name: &str) -> String {
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", name));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    form.push(p("VpcId", "vpc-12345678"));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create {name}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    reply
        .text("TargetGroupArn")
        .unwrap_or_else(|| panic!("case create {name}: no TargetGroupArn in {}", reply.body))
}

/// Create a forwarding HTTP listener and answer its ARN.
async fn create_forward_listener(
    router: &MultiServiceRouter,
    lb_arn: &str,
    tg_arn: &str,
    port: i32,
) -> String {
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", lb_arn));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", &port.to_string()));
    form.push(p("DefaultActions.member.1.Type", "forward"));
    form.push(p("DefaultActions.member.1.TargetGroupArn", tg_arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create listener on port {port}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    reply
        .text("ListenerArn")
        .unwrap_or_else(|| panic!("case create listener: no ListenerArn in {}", reply.body))
}

fn is_hex_id(value: &str) -> bool {
    value.len() == 16 && value.chars().all(|c| c.is_ascii_hexdigit())
}

// ---------------------------------------------------------------------------
// The loop the issue describes, end to end.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_provisioning_loop_is_persisted_and_read_back() {
    let router = router();

    let lb_arn = create_alb(&router, "my-alb").await;
    let expected_prefix = format!(
        "arn:aws:elasticloadbalancing:{REGION}:{}:loadbalancer/app/my-alb/",
        account()
    );
    assert!(
        lb_arn.starts_with(&expected_prefix),
        "case alb arn: expected prefix {expected_prefix}, got {lb_arn}"
    );
    let lb_id = lb_arn[expected_prefix.len()..].to_string();
    assert!(
        is_hex_id(&lb_id),
        "case alb arn: expected a 16-hex id, got {lb_id:?}"
    );

    // Read the load balancer back: every field must come from what was stored.
    let mut form = action("DescribeLoadBalancers");
    form.push(p("LoadBalancerArns.member.1", &lb_arn));
    let described = call(&router, &form).await;
    assert_eq!(
        described.status, 200,
        "case describe by arn: expected 200, got {} body {}",
        described.status, described.body
    );
    let found = members(&described.body, "LoadBalancers");
    assert_eq!(
        found.len(),
        1,
        "case describe by arn: expected exactly 1 load balancer, got {} in {}",
        found.len(),
        described.body
    );
    let lb = &found[0];
    assert_eq!(
        first_text(lb, "LoadBalancerName").as_deref(),
        Some("my-alb"),
        "case describe by arn: expected the created name, got {:?}",
        first_text(lb, "LoadBalancerName")
    );
    // The address an Application Load Balancer publishes: the `host:port` of the socket
    // this emulator opened for it, which is an address a client that honours `DNSName` can
    // dial. The port is assigned by the core, so what is asserted here is that the value
    // IS such an address; the per-family forms are tabled in
    // `load_balancer_dns_name_form.rs`.
    let dns_name = first_text(lb, "DNSName").unwrap_or_default();
    assert!(
        dns_name.parse::<std::net::SocketAddr>().is_ok(),
        "case describe by arn: expected an address a client can dial, got {dns_name:?}"
    );
    assert_eq!(
        first_text(lb, "Scheme").as_deref(),
        Some("internet-facing"),
        "case describe by arn: expected the default scheme, got {:?}",
        first_text(lb, "Scheme")
    );
    assert_eq!(
        first_text(lb, "Type").as_deref(),
        Some("application"),
        "case describe by arn: expected the default type, got {:?}",
        first_text(lb, "Type")
    );
    let state = first_text(lb, "Code").unwrap_or_default();
    assert!(
        state == "provisioning" || state == "active",
        "case describe by arn: expected provisioning or active, got {state:?}"
    );
    let created = first_text(lb, "CreatedTime").unwrap_or_default();
    assert!(
        created.contains('T'),
        "case describe by arn: expected an ISO-8601 CreatedTime, got {created:?}"
    );
    let hosted_zone = first_text(lb, "CanonicalHostedZoneId");
    assert_eq!(
        hosted_zone.as_deref(),
        // The hosted zone Elastic Load Balancing publishes for an Application Load
        // Balancer of this Region; `all/load_balancer_input_rules.rs` walks the rest of
        // that table.
        Some("Z1H1FL5HABSF5"),
        "case describe by arn: expected the hosted zone published for {REGION}, got {hosted_zone:?}"
    );
    let subnets: Vec<String> = members(lb, "AvailabilityZones")
        .iter()
        .filter_map(|zone| first_text(zone, "SubnetId"))
        .collect();
    assert_eq!(
        subnets,
        vec!["subnet-11111111".to_string(), "subnet-22222222".to_string()],
        "case describe by arn: expected both requested subnets, got {subnets:?}"
    );

    // The same record must come back by name.
    let mut form = action("DescribeLoadBalancers");
    form.push(p("Names.member.1", "my-alb"));
    let by_name = call(&router, &form).await;
    assert_eq!(
        by_name.text("LoadBalancerArn").as_deref(),
        Some(lb_arn.as_str()),
        "case describe by name: expected the same ARN, got {:?}",
        by_name.text("LoadBalancerArn")
    );

    // A target group, then targets, read back through DescribeTargetHealth.
    let tg_arn = create_http_target_group(&router, "my-tg").await;
    let tg_prefix = format!(
        "arn:aws:elasticloadbalancing:{REGION}:{}:targetgroup/my-tg/",
        account()
    );
    assert!(
        tg_arn.starts_with(&tg_prefix),
        "case target group arn: expected prefix {tg_prefix}, got {tg_arn}"
    );

    let mut form = action("RegisterTargets");
    form.push(p("TargetGroupArn", &tg_arn));
    form.push(p("Targets.member.1.Id", "i-11111111111111111"));
    form.push(p("Targets.member.2.Id", "i-22222222222222222"));
    form.push(p("Targets.member.2.Port", "8080"));
    let registered = call(&router, &form).await;
    assert_eq!(
        registered.status, 200,
        "case register targets: expected 200, got {} body {}",
        registered.status, registered.body
    );

    // No load balancer forwards to the group yet, so its targets are unused — the
    // state is derived from what is stored, not canned.
    let mut form = action("DescribeTargetHealth");
    form.push(p("TargetGroupArn", &tg_arn));
    let health = call(&router, &form).await;
    let descriptions = members(&health.body, "TargetHealthDescriptions");
    assert_eq!(
        descriptions.len(),
        2,
        "case health before a listener: expected both registrations, got {}",
        health.body
    );
    for description in &descriptions {
        assert_eq!(
            first_text(description, "State").as_deref(),
            Some("unused"),
            "case health before a listener: expected unused, got {:?}",
            first_text(description, "State")
        );
        assert_eq!(
            first_text(description, "Reason").as_deref(),
            Some("Target.NotInUse"),
            "case health before a listener: expected Target.NotInUse, got {:?}",
            first_text(description, "Reason")
        );
    }
    let registrations: Vec<(Option<String>, Option<String>)> = descriptions
        .iter()
        .map(|d| (first_text(d, "Id"), first_text(d, "Port")))
        .collect();
    assert!(
        registrations.contains(&(Some("i-11111111111111111".to_string()), Some("80".to_string()))),
        "case health: a target registered without a port takes the group's port, got {registrations:?}"
    );
    assert!(
        registrations.contains(&(
            Some("i-22222222222222222".to_string()),
            Some("8080".to_string())
        )),
        "case health: a target registered with a port keeps it, got {registrations:?}"
    );

    // A forwarding listener associates the group with the load balancer.
    let listener_arn = create_forward_listener(&router, &lb_arn, &tg_arn, 80).await;
    let listener_prefix = format!(
        "arn:aws:elasticloadbalancing:{REGION}:{}:listener/app/my-alb/{lb_id}/",
        account()
    );
    assert!(
        listener_arn.starts_with(&listener_prefix),
        "case listener arn: expected prefix {listener_prefix}, got {listener_arn}"
    );

    let mut form = action("DescribeListeners");
    form.push(p("LoadBalancerArn", &lb_arn));
    let listeners = call(&router, &form).await;
    assert_eq!(
        listeners.text("ListenerArn").as_deref(),
        Some(listener_arn.as_str()),
        "case describe listeners: expected the created listener, got {:?}",
        listeners.text("ListenerArn")
    );
    assert_eq!(
        listeners.text("TargetGroupArn").as_deref(),
        Some(tg_arn.as_str()),
        "case describe listeners: expected the forward action's target group, got {:?}",
        listeners.text("TargetGroupArn")
    );
    assert_eq!(
        listeners.text("Port").as_deref(),
        Some("80"),
        "case describe listeners: expected port 80, got {:?}",
        listeners.text("Port")
    );

    let mut form = action("DescribeTargetGroups");
    form.push(p("Names.member.1", "my-tg"));
    let groups = call(&router, &form).await;
    let associated: Vec<String> = members(&groups.body, "LoadBalancerArns");
    assert_eq!(
        associated,
        vec![lb_arn.clone()],
        "case target group association: expected the listener's load balancer, got {associated:?}"
    );

    // With the association in place the targets are coming up or up, never unused.
    let mut form = action("DescribeTargetHealth");
    form.push(p("TargetGroupArn", &tg_arn));
    let health = call(&router, &form).await;
    for description in members(&health.body, "TargetHealthDescriptions") {
        let state = first_text(&description, "State").unwrap_or_default();
        assert!(
            state == "initial" || state == "healthy",
            "case health after a listener: expected initial or healthy, got {state:?}"
        );
    }

    // A target that was never registered is reported as such.
    let mut form = action("DescribeTargetHealth");
    form.push(p("TargetGroupArn", &tg_arn));
    form.push(p("Targets.member.1.Id", "i-99999999999999999"));
    let health = call(&router, &form).await;
    assert_eq!(
        health.text("Reason").as_deref(),
        Some("Target.NotRegistered"),
        "case unregistered target: expected Target.NotRegistered, got {}",
        health.body
    );

    // The clock owes the load balancer its move out of `provisioning` and each
    // registration its move out of `initial`; once the span has passed, a read
    // materializes both without anything having written. The workspace's `tokio`
    // carries no `time` feature, so this waits on the std clock rather than
    // `tokio::time::sleep`; nothing else is pending on this runtime.
    std::thread::sleep(std::time::Duration::from_millis(800));
    let mut form = action("DescribeLoadBalancers");
    form.push(p("LoadBalancerArns.member.1", &lb_arn));
    let settled = call(&router, &form).await;
    assert_eq!(
        settled.text("Code").as_deref(),
        Some("active"),
        "case settled state: expected active once the provisioning span has passed, got {}",
        settled.body
    );
    let mut form = action("DescribeTargetHealth");
    form.push(p("TargetGroupArn", &tg_arn));
    let health = call(&router, &form).await;
    for description in members(&health.body, "TargetHealthDescriptions") {
        assert_eq!(
            first_text(&description, "State").as_deref(),
            Some("healthy"),
            "case settled health: expected healthy once the registration span has passed, got {}",
            health.body
        );
    }

    // Teardown: the delete is observable, and the target group survives it.
    let mut form = action("DeleteLoadBalancer");
    form.push(p("LoadBalancerArn", &lb_arn));
    let deleted = call(&router, &form).await;
    assert_eq!(
        deleted.status, 200,
        "case delete: expected 200, got {} body {}",
        deleted.status, deleted.body
    );

    let mut form = action("DescribeLoadBalancers");
    form.push(p("LoadBalancerArns.member.1", &lb_arn));
    let gone = call(&router, &form).await;
    assert_eq!(
        (gone.status, gone.error_code().as_str()),
        (400, "LoadBalancerNotFound"),
        "case describe after delete: expected 400 LoadBalancerNotFound, got {} {}",
        gone.status,
        gone.body
    );

    let mut form = action("DescribeListeners");
    form.push(p("ListenerArns.member.1", &listener_arn));
    let listener_gone = call(&router, &form).await;
    assert_eq!(
        (listener_gone.status, listener_gone.error_code().as_str()),
        (400, "ListenerNotFound"),
        "case listener after delete: expected 400 ListenerNotFound, got {} {}",
        listener_gone.status,
        listener_gone.body
    );

    let mut form = action("DescribeTargetGroups");
    form.push(p("Names.member.1", "my-tg"));
    let groups = call(&router, &form).await;
    assert_eq!(
        groups.status, 200,
        "case target group after delete: expected it to survive, got {} {}",
        groups.status, groups.body
    );
    let still_associated = members(&groups.body, "LoadBalancerArns");
    assert!(
        still_associated.is_empty(),
        "case target group after delete: expected no association left, got {}",
        groups.body
    );
    let mut form = action("DescribeTargetHealth");
    form.push(p("TargetGroupArn", &tg_arn));
    let health = call(&router, &form).await;
    assert_eq!(
        members(&health.body, "TargetHealthDescriptions").len(),
        2,
        "case targets after delete: deleting a load balancer does not affect its registered targets, got {}",
        health.body
    );
}

// ---------------------------------------------------------------------------
// Identity: the ARN, the DNS name and the type code follow name, scheme and type.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn load_balancer_identity_follows_name_scheme_and_type() {
    // `Type` and `Scheme` values are the model's `LoadBalancerTypeEnum` /
    // `LoadBalancerSchemeEnum`; the ARN segment is the one AWS publishes per type. A Gateway
    // Load Balancer names NO scheme — "you can't specify a scheme for a Gateway Load
    // Balancer" — and carries none, so its row expects no `Scheme` reported back.
    let cases = [
        ("alb-public", "application", Some("internet-facing"), "app", 2),
        ("alb-private", "application", Some("internal"), "app", 2),
        ("nlb-public", "network", Some("internet-facing"), "net", 1),
        ("nlb-private", "network", Some("internal"), "net", 3),
        ("glb-one", "gateway", None, "gwy", 1),
    ];

    let router = router();
    let mut seen_ids: Vec<String> = Vec::new();

    for (name, lb_type, scheme, code, subnet_count) in cases {
        let mut form = action("CreateLoadBalancer");
        form.push(p("Name", name));
        form.push(p("Type", lb_type));
        if let Some(scheme) = scheme {
            form.push(p("Scheme", scheme));
        }
        for index in 1..=subnet_count {
            form.push(p(
                &format!("Subnets.member.{index}"),
                &format!("subnet-{index}{index}{index}"),
            ));
        }
        let created = call(&router, &form).await;
        assert_eq!(
            created.status, 200,
            "case {name}/{lb_type}/{scheme:?}: expected 200, got {} body {}",
            created.status, created.body
        );

        // Read it back rather than trusting the create's own answer.
        let mut form = action("DescribeLoadBalancers");
        form.push(p("Names.member.1", name));
        let reply = call(&router, &form).await;
        let arn = reply.text("LoadBalancerArn").unwrap_or_default();
        let prefix = format!(
            "arn:aws:elasticloadbalancing:{REGION}:{}:loadbalancer/{code}/{name}/",
            account()
        );
        assert!(
            arn.starts_with(&prefix),
            "case {name}/{lb_type}/{scheme:?}: expected ARN prefix {prefix}, got {arn}"
        );
        let id = arn[prefix.len()..].to_string();
        assert!(
            is_hex_id(&id),
            "case {name}/{lb_type}/{scheme:?}: expected a 16-hex id, got {id:?}"
        );
        assert!(
            !seen_ids.contains(&id),
            "case {name}/{lb_type}/{scheme:?}: ids must be unique within a run, {id} repeated"
        );
        seen_ids.push(id.clone());

        // What each load balancer publishes as its `DNSName`: a Gateway Load Balancer's
        // `GENEVE` listeners carry no traffic of this kind, so it keeps the host Elastic Load
        // Balancing's own zone gives it (the ARN's own id, the `elb` label ahead of the
        // Region, no scheme stated in it), while an Application or Network Load Balancer —
        // whose listeners carry a client's own traffic — publishes the address of the socket
        // this emulator opened for it, an address a client can dial, whatever its scheme. The
        // whole table of forms is `load_balancer_dns_name_form.rs`.
        let dns_name = reply.text("DNSName").unwrap_or_default();
        if lb_type == "gateway" {
            let host = format!("{name}-{id}.elb.{REGION}.amazonaws.com");
            assert_eq!(
                dns_name, host,
                "case {name}/{lb_type}/{scheme:?}: unexpected DNS name {dns_name:?}"
            );
        } else {
            assert!(
                dns_name.parse::<std::net::SocketAddr>().is_ok(),
                "case {name}/{lb_type}/{scheme:?}: expected an address a client can dial, got \
                 {dns_name:?}"
            );
        }
        assert_eq!(
            reply.text("Type").as_deref(),
            Some(lb_type),
            "case {name}/{lb_type}/{scheme:?}: unexpected Type {:?}",
            reply.text("Type")
        );
        // The scheme the load balancer carries — and NO `Scheme` member at all for the
        // family that carries none.
        assert_eq!(
            reply.text("Scheme").as_deref(),
            scheme,
            "case {name}/{lb_type}/{scheme:?}: unexpected Scheme {:?}",
            reply.text("Scheme")
        );

        let found = members(&reply.body, "LoadBalancers");
        assert_eq!(
            found.len(),
            1,
            "case {name}/{lb_type}/{scheme:?}: expected exactly 1 load balancer, got {}",
            reply.body
        );
        let zone_names: Vec<String> = members(&found[0], "AvailabilityZones")
            .iter()
            .filter_map(|zone| first_text(zone, "ZoneName"))
            .collect();
        assert_eq!(
            zone_names.len(),
            subnet_count,
            "case {name}/{lb_type}/{scheme:?}: expected one zone per subnet, got {zone_names:?}"
        );
        for zone in &zone_names {
            assert!(
                zone.starts_with(REGION),
                "case {name}/{lb_type}/{scheme:?}: a zone must be in the request's region, got \
                 {zone}"
            );
        }
        let mut distinct = zone_names.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            zone_names.len(),
            "case {name}/{lb_type}/{scheme:?}: one subnet per zone, got {zone_names:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Target group health-check defaults, one row per rule the model states.
// ---------------------------------------------------------------------------

/// What a created target group must report for its health check.
struct HealthCheckExpectation {
    protocol: Option<&'static str>,
    port: Option<&'static str>,
    path: Option<&'static str>,
    interval: &'static str,
    timeout: &'static str,
    healthy: &'static str,
    unhealthy: &'static str,
    http_matcher: Option<&'static str>,
    grpc_matcher: Option<&'static str>,
    enabled: &'static str,
    target_type: &'static str,
}

#[tokio::test]
async fn target_group_health_check_defaults_follow_the_model() {
    // Each row is a sentence the model itself carries for those members (see
    // `smithy_query(query="doc", shape="CreateTargetGroupInput",
    // member="HealthCheck…")`, quoted in `src/ops/create_target_group.rs`).
    let cases: Vec<HealthCheckCase> = vec![
        (
            // HTTP: health check HTTP, path /, interval 30, timeout 6, 5/2, 200.
            "http-instance",
            vec![p("Protocol", "HTTP"), p("Port", "80"), p("VpcId", "vpc-1")],
            HealthCheckExpectation {
                protocol: Some("HTTP"),
                port: Some("traffic-port"),
                path: Some("/"),
                interval: "30",
                // The RECORDED answer for an HTTP target group, which the model's own
                // "the default is 6 seconds" disagrees with; see
                // `health_check_config` in `src/ops/create_target_group.rs`.
                timeout: "5",
                healthy: "5",
                unhealthy: "2",
                http_matcher: Some("200"),
                grpc_matcher: None,
                enabled: "true",
                target_type: "instance",
            },
        ),
        (
            // HTTPS: the health check speaks the target group's own protocol, and takes
            // the same 5-second timeout an HTTP group's does — RECORDED behaviour, which
            // the member's own "for Application Load Balancers, the default is HTTP" and
            // "for target groups with a protocol of TCP, TLS or HTTPS, the default is 10
            // seconds" disagree with. The whole default set of each protocol is tabled in
            // `all/target_group_protocol_version_and_health_check_defaults.rs`.
            "https-instance",
            vec![
                p("Protocol", "HTTPS"),
                p("Port", "443"),
                p("VpcId", "vpc-1"),
            ],
            HealthCheckExpectation {
                protocol: Some("HTTPS"),
                port: Some("traffic-port"),
                path: Some("/"),
                interval: "30",
                timeout: "5",
                healthy: "5",
                unhealthy: "2",
                http_matcher: Some("200"),
                grpc_matcher: None,
                enabled: "true",
                target_type: "instance",
            },
        ),
        (
            // TCP: a Network Load Balancer's group, so the health check is TCP and
            // has neither a path nor a matcher.
            "tcp-instance",
            vec![
                p("Protocol", "TCP"),
                p("Port", "1234"),
                p("VpcId", "vpc-1"),
            ],
            HealthCheckExpectation {
                protocol: Some("TCP"),
                port: Some("traffic-port"),
                path: None,
                interval: "30",
                timeout: "10",
                healthy: "5",
                unhealthy: "2",
                http_matcher: None,
                grpc_matcher: None,
                enabled: "true",
                target_type: "instance",
            },
        ),
        (
            // GENEVE: interval 10, timeout 5, health-check port 80.
            "geneve-group",
            vec![
                p("Protocol", "GENEVE"),
                p("Port", "6081"),
                p("VpcId", "vpc-1"),
            ],
            HealthCheckExpectation {
                protocol: Some("TCP"),
                port: Some("80"),
                path: None,
                interval: "10",
                timeout: "5",
                healthy: "5",
                unhealthy: "2",
                http_matcher: None,
                grpc_matcher: None,
                enabled: "true",
                target_type: "instance",
            },
        ),
        (
            // gRPC over HTTP: path /AWS.ALB/healthcheck and the gRPC success code.
            "grpc-group",
            vec![
                p("Protocol", "HTTP"),
                p("ProtocolVersion", "GRPC"),
                p("Port", "80"),
                p("VpcId", "vpc-1"),
            ],
            HealthCheckExpectation {
                protocol: Some("HTTP"),
                port: Some("traffic-port"),
                path: Some("/AWS.ALB/healthcheck"),
                interval: "30",
                timeout: "5",
                healthy: "5",
                unhealthy: "2",
                http_matcher: None,
                grpc_matcher: Some("12"),
                enabled: "true",
                target_type: "instance",
            },
        ),
        (
            // lambda: interval 35, timeout 30, unhealthy 2, health checks off. A
            // Lambda target group reports NO health-check protocol or port — the load
            // balancer invokes the function rather than connecting to a port — and its
            // health check is an HTTP one all the same, so it carries the documented
            // path and matcher. (The recorded AWS response for
            // `elasticloadbalancing-lambda-target-group-lifecycle` diverged on exactly
            // `HealthCheckPath`, `Matcher` and `IpAddressType` while
            // `HealthCheckProtocol` and `HealthCheckPort` matched their absence.)
            "lambda-group",
            vec![p("TargetType", "lambda")],
            HealthCheckExpectation {
                protocol: None,
                port: None,
                path: Some("/"),
                interval: "35",
                timeout: "30",
                healthy: "5",
                unhealthy: "2",
                http_matcher: Some("200"),
                grpc_matcher: None,
                enabled: "false",
                target_type: "lambda",
            },
        ),
        (
            // Supplied values win over every default.
            "explicit-group",
            vec![
                p("Protocol", "HTTP"),
                p("Port", "80"),
                p("VpcId", "vpc-1"),
                p("HealthCheckProtocol", "HTTPS"),
                p("HealthCheckPort", "8443"),
                p("HealthCheckPath", "/ping"),
                p("HealthCheckIntervalSeconds", "17"),
                p("HealthCheckTimeoutSeconds", "11"),
                p("HealthyThresholdCount", "3"),
                p("UnhealthyThresholdCount", "4"),
                p("Matcher.HttpCode", "200-299"),
            ],
            HealthCheckExpectation {
                protocol: Some("HTTPS"),
                port: Some("8443"),
                path: Some("/ping"),
                interval: "17",
                timeout: "11",
                healthy: "3",
                unhealthy: "4",
                http_matcher: Some("200-299"),
                grpc_matcher: None,
                enabled: "true",
                target_type: "instance",
            },
        ),
    ];

    let router = router();

    for (name, params, expected) in cases {
        let mut form = action("CreateTargetGroup");
        form.push(p("Name", name));
        form.extend(params);
        let created = call(&router, &form).await;
        assert_eq!(
            created.status, 200,
            "case {name}: expected 200, got {} body {}",
            created.status, created.body
        );

        // Read it back: the defaults must be PERSISTED, not just answered once.
        let mut form = action("DescribeTargetGroups");
        form.push(p("Names.member.1", name));
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, 200,
            "case {name}: expected 200 on read-back, got {} body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.text("HealthCheckProtocol").as_deref(),
            expected.protocol,
            "case {name}: unexpected HealthCheckProtocol in {}",
            reply.body
        );
        assert_eq!(
            reply.text("HealthCheckPort").as_deref(),
            expected.port,
            "case {name}: unexpected HealthCheckPort in {}",
            reply.body
        );
        assert_eq!(
            reply.text("HealthCheckPath").as_deref(),
            expected.path,
            "case {name}: unexpected HealthCheckPath in {}",
            reply.body
        );
        assert_eq!(
            reply.text("HealthCheckIntervalSeconds").as_deref(),
            Some(expected.interval),
            "case {name}: unexpected HealthCheckIntervalSeconds in {}",
            reply.body
        );
        assert_eq!(
            reply.text("HealthCheckTimeoutSeconds").as_deref(),
            Some(expected.timeout),
            "case {name}: unexpected HealthCheckTimeoutSeconds in {}",
            reply.body
        );
        assert_eq!(
            reply.text("HealthyThresholdCount").as_deref(),
            Some(expected.healthy),
            "case {name}: unexpected HealthyThresholdCount in {}",
            reply.body
        );
        assert_eq!(
            reply.text("UnhealthyThresholdCount").as_deref(),
            Some(expected.unhealthy),
            "case {name}: unexpected UnhealthyThresholdCount in {}",
            reply.body
        );
        assert_eq!(
            reply.text("HttpCode").as_deref(),
            expected.http_matcher,
            "case {name}: unexpected Matcher HttpCode in {}",
            reply.body
        );
        assert_eq!(
            reply.text("GrpcCode").as_deref(),
            expected.grpc_matcher,
            "case {name}: unexpected Matcher GrpcCode in {}",
            reply.body
        );
        assert_eq!(
            reply.text("HealthCheckEnabled").as_deref(),
            Some(expected.enabled),
            "case {name}: unexpected HealthCheckEnabled in {}",
            reply.body
        );
        assert_eq!(
            reply.text("TargetType").as_deref(),
            Some(expected.target_type),
            "case {name}: unexpected TargetType in {}",
            reply.body
        );
        assert!(
            reply.text("LoadBalancerArns").is_none(),
            "case {name}: a target group no listener forwards to has no load balancers, got {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn health_check_values_are_accepted_on_their_published_bounds_and_refused_past_them() {
    // The bounds are the model's own ranges: interval 5..300, timeout 2..120,
    // thresholds 2..10 (`smithy_query(query="operation",
    // operation="CreateTargetGroup")`).
    let accepted = [
        ("interval-low", "HealthCheckIntervalSeconds", "5"),
        ("interval-high", "HealthCheckIntervalSeconds", "300"),
        ("timeout-low", "HealthCheckTimeoutSeconds", "2"),
        ("timeout-high", "HealthCheckTimeoutSeconds", "120"),
        ("healthy-low", "HealthyThresholdCount", "2"),
        ("healthy-high", "HealthyThresholdCount", "10"),
        ("unhealthy-low", "UnhealthyThresholdCount", "2"),
        ("unhealthy-high", "UnhealthyThresholdCount", "10"),
    ];
    let refused = [
        ("interval-under", "HealthCheckIntervalSeconds", "4"),
        ("interval-over", "HealthCheckIntervalSeconds", "301"),
        ("timeout-under", "HealthCheckTimeoutSeconds", "1"),
        ("timeout-over", "HealthCheckTimeoutSeconds", "121"),
        ("healthy-over", "HealthyThresholdCount", "11"),
        ("unhealthy-under", "UnhealthyThresholdCount", "1"),
    ];

    let router = router();

    for (case, member, value) in accepted {
        let mut form = action("CreateTargetGroup");
        form.push(p("Name", case));
        form.push(p("Protocol", "HTTP"));
        form.push(p("Port", "80"));
        form.push(p("VpcId", "vpc-1"));
        form.push(p(member, value));
        // A create settles the WHOLE health check, and its interval has to exceed its
        // timeout, so a row moving one of those two members to a published bound names
        // the other at its far bound: what this table is about is the bound of ONE
        // member, and the schedule rule itself is the subject of
        // `target_group_health_check_schedule`.
        match member {
            "HealthCheckIntervalSeconds" => form.push(p("HealthCheckTimeoutSeconds", "2")),
            "HealthCheckTimeoutSeconds" => form.push(p("HealthCheckIntervalSeconds", "300")),
            _ => {}
        }
        let created = call(&router, &form).await;
        assert_eq!(
            created.status, 200,
            "case {case}: {member}={value} is on a published bound and must be accepted, got {} {}",
            created.status, created.body
        );
        let mut form = action("DescribeTargetGroups");
        form.push(p("Names.member.1", case));
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.text(member).as_deref(),
            Some(value),
            "case {case}: expected the stored {member} to be {value}, got {}",
            reply.body
        );
    }

    for (case, member, value) in refused {
        let mut form = action("CreateTargetGroup");
        form.push(p("Name", case));
        form.push(p("Protocol", "HTTP"));
        form.push(p("Port", "80"));
        form.push(p("VpcId", "vpc-1"));
        form.push(p(member, value));
        let created = call(&router, &form).await;
        assert_eq!(
            created.status, 400,
            "case {case}: {member}={value} is outside the published range and must be refused, got {} {}",
            created.status, created.body
        );
        // Nothing may be persisted by a refused create.
        let mut form = action("DescribeTargetGroups");
        form.push(p("Names.member.1", case));
        let reply = call(&router, &form).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "TargetGroupNotFound"),
            "case {case}: a refused create must store nothing, got {} {}",
            reply.status,
            reply.body
        );
    }
}

// ---------------------------------------------------------------------------
// Idempotent repeats versus duplicates.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_repeat_with_the_same_settings_is_idempotent_and_a_different_one_is_a_duplicate() {
    let router = router();

    // Load balancer: same name, same settings -> the stored resource.
    let first = create_alb(&router, "dup-alb").await;
    let second = create_alb(&router, "dup-alb").await;
    assert_eq!(
        first, second,
        "case load balancer repeat: the model documents the operation as idempotent, got {first} then {second}"
    );
    let described = call(&router, &action("DescribeLoadBalancers")).await;
    assert_eq!(
        members(&described.body, "LoadBalancers").len(),
        1,
        "case load balancer repeat: an idempotent repeat must mint nothing, got {}",
        described.body
    );

    // Load balancer: same name, different settings -> a duplicate.
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", "dup-alb"));
    form.push(p("Scheme", "internal"));
    form.push(p("Subnets.member.1", "subnet-11111111"));
    form.push(p("Subnets.member.2", "subnet-22222222"));
    let conflict = call(&router, &form).await;
    assert_eq!(
        (conflict.status, conflict.error_code().as_str()),
        (400, "DuplicateLoadBalancerName"),
        "case load balancer conflict: expected 400 DuplicateLoadBalancerName, got {} {}",
        conflict.status,
        conflict.body
    );

    // Target group: same settings -> the stored resource; different port -> duplicate.
    let tg_first = create_http_target_group(&router, "dup-tg").await;
    let tg_second = create_http_target_group(&router, "dup-tg").await;
    assert_eq!(
        tg_first, tg_second,
        "case target group repeat: expected the stored target group, got {tg_first} then {tg_second}"
    );
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", "dup-tg"));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "8080"));
    form.push(p("VpcId", "vpc-12345678"));
    let conflict = call(&router, &form).await;
    assert_eq!(
        (conflict.status, conflict.error_code().as_str()),
        (400, "DuplicateTargetGroupName"),
        "case target group conflict: expected 400 DuplicateTargetGroupName, got {} {}",
        conflict.status,
        conflict.body
    );

    // Listener: same port and settings -> the stored resource; a different protocol
    // on the same port -> duplicate.
    let listener_first = create_forward_listener(&router, &first, &tg_first, 80).await;
    let listener_second = create_forward_listener(&router, &first, &tg_first, 80).await;
    assert_eq!(
        listener_first, listener_second,
        "case listener repeat: expected the stored listener, got {listener_first} then {listener_second}"
    );
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", &first));
    form.push(p("Protocol", "HTTPS"));
    form.push(p("Port", "80"));
    // An HTTPS listener carries the default certificate the model brackets for it, so
    // this row reaches the duplicate check rather than the certificate refusal pinned
    // in `listener_tls_certificates.rs`.
    form.push(p(
        "Certificates.member.1.CertificateArn",
        "arn:aws:acm:us-west-2:000000000000:certificate/dup-listener",
    ));
    form.push(p("DefaultActions.member.1.Type", "forward"));
    form.push(p("DefaultActions.member.1.TargetGroupArn", &tg_first));
    let conflict = call(&router, &form).await;
    assert_eq!(
        (conflict.status, conflict.error_code().as_str()),
        (400, "DuplicateListener"),
        "case listener conflict: expected 400 DuplicateListener, got {} {}",
        conflict.status,
        conflict.body
    );

    // A second listener on another port is not a duplicate.
    let other_port = create_forward_listener(&router, &first, &tg_first, 8080).await;
    assert_ne!(
        other_port, listener_first,
        "case listener on another port: expected a new listener, got {other_port}"
    );
    let mut form = action("DescribeListeners");
    form.push(p("LoadBalancerArn", &first));
    let listeners = call(&router, &form).await;
    assert_eq!(
        members(&listeners.body, "Listeners").len(),
        2,
        "case listeners after two ports: expected 2, got {}",
        listeners.body
    );
}

// ---------------------------------------------------------------------------
// Pagination.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn describe_load_balancers_pages_by_page_size_and_resumes_from_the_marker() {
    let router = router();
    let mut created = Vec::new();
    for index in 1..=5 {
        created.push(create_alb(&router, &format!("page-alb-{index}")).await);
    }
    created.sort();

    for page_size in [1usize, 2, 3, 5] {
        let mut collected: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut form = action("DescribeLoadBalancers");
            form.push(p("PageSize", &page_size.to_string()));
            if let Some(marker) = marker.as_deref() {
                form.push(p("Marker", marker));
            }
            let reply = call(&router, &form).await;
            assert_eq!(
                reply.status, 200,
                "case page size {page_size}: expected 200, got {} body {}",
                reply.status, reply.body
            );
            let page = members(&reply.body, "LoadBalancers");
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: a page must not exceed it, got {}",
                page.len()
            );
            for lb in &page {
                collected.push(first_text(lb, "LoadBalancerArn").unwrap_or_default());
            }
            pages += 1;
            assert!(
                pages <= 10,
                "case page size {page_size}: pagination did not terminate after {pages} pages"
            );
            marker = reply.text("NextMarker");
            if marker.is_none() {
                break;
            }
        }
        let mut sorted = collected.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted, created,
            "case page size {page_size}: every load balancer must appear exactly once across the pages, got {collected:?}"
        );
    }

    // A marker that is not a position in this collection is refused rather than
    // silently served.
    let mut form = action("DescribeLoadBalancers");
    form.push(p("Marker", "not-a-marker"));
    let refused = call(&router, &form).await;
    assert_eq!(
        refused.status, 400,
        "case bogus marker: expected 400, got {} body {}",
        refused.status, refused.body
    );

    // So is one past the end of the collection.
    let mut form = action("DescribeLoadBalancers");
    form.push(p("Marker", "99"));
    let past_end = call(&router, &form).await;
    assert_eq!(
        past_end.status, 400,
        "case marker past the end: expected 400, got {} body {}",
        past_end.status, past_end.body
    );
}

// ---------------------------------------------------------------------------
// Subnets versus subnet mappings.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subnet_mappings_stand_in_for_subnets_and_mixing_or_omitting_both_is_refused() {
    let router = router();

    // Mappings only: one zone per mapping, and what the caller pinned on it is
    // reported back.
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", "mapped-nlb"));
    form.push(p("Type", "network"));
    form.push(p("SubnetMappings.member.1.SubnetId", "subnet-aaaaaaa1"));
    form.push(p("SubnetMappings.member.1.AllocationId", "eipalloc-1111"));
    form.push(p("SubnetMappings.member.2.SubnetId", "subnet-aaaaaaa2"));
    let created = call(&router, &form).await;
    assert_eq!(
        created.status, 200,
        "case mappings only: expected 200, got {} body {}",
        created.status, created.body
    );

    let mut form = action("DescribeLoadBalancers");
    form.push(p("Names.member.1", "mapped-nlb"));
    let reply = call(&router, &form).await;
    let lb = members(&reply.body, "LoadBalancers");
    assert_eq!(
        lb.len(),
        1,
        "case mappings only: expected exactly 1 load balancer, got {}",
        reply.body
    );
    let subnets: Vec<String> = members(&lb[0], "AvailabilityZones")
        .iter()
        .filter_map(|zone| first_text(zone, "SubnetId"))
        .collect();
    assert_eq!(
        subnets,
        vec![
            "subnet-aaaaaaa1".to_string(),
            "subnet-aaaaaaa2".to_string()
        ],
        "case mappings only: expected both mapped subnets, got {subnets:?}"
    );
    assert_eq!(
        reply.text("AllocationId").as_deref(),
        Some("eipalloc-1111"),
        "case mappings only: expected the pinned allocation id, got {}",
        reply.body
    );

    // "You must specify either subnets or subnet mappings, but not both."
    let refusals: Vec<(&str, Form)> = vec![
        (
            "both subnets and subnet mappings",
            vec![
                p("Action", "CreateLoadBalancer"),
                p("Version", VERSION),
                p("Name", "mixed-alb"),
                p("Subnets.member.1", "subnet-11111111"),
                p("SubnetMappings.member.1.SubnetId", "subnet-22222222"),
            ],
        ),
        (
            "neither subnets nor subnet mappings",
            vec![
                p("Action", "CreateLoadBalancer"),
                p("Version", VERSION),
                p("Name", "bare-alb"),
            ],
        ),
    ];

    for (case, form) in refusals {
        let reply = call(&router, &form).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "InvalidConfigurationRequest"),
            "case {case}: expected a 400 refusal, got {} {}",
            reply.status,
            reply.body
        );
    }

    // And nothing was stored by either refused create.
    for name in ["mixed-alb", "bare-alb"] {
        let mut form = action("DescribeLoadBalancers");
        form.push(p("Names.member.1", name));
        let reply = call(&router, &form).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "LoadBalancerNotFound"),
            "case {name}: a refused create must store nothing, got {} {}",
            reply.status,
            reply.body
        );
    }
}

// ---------------------------------------------------------------------------
// Health checks that cannot be turned off.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health_checks_can_be_disabled_only_for_a_lambda_target_group() {
    // The model: "If the target type is lambda, health checks are disabled by
    // default but can be enabled. If the target type is instance, ip, or alb,
    // health checks are always enabled and can't be disabled."
    let refused = ["instance", "ip", "alb"];
    let router = router();

    for target_type in refused {
        let mut form = action("CreateTargetGroup");
        form.push(p("Name", &format!("no-hc-{target_type}")));
        form.push(p("Protocol", "HTTP"));
        form.push(p("Port", "80"));
        form.push(p("VpcId", "vpc-1"));
        form.push(p("TargetType", target_type));
        form.push(p("HealthCheckEnabled", "false"));
        let reply = call(&router, &form).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "InvalidConfigurationRequest"),
            "case {target_type}: health checks cannot be disabled, got {} {}",
            reply.status,
            reply.body
        );
        // The refusal states the requirement the member has for that kind of target group,
        // naming the target type the request asked for — the same sentence whichever of
        // them it is.
        let expected = format!(
            "Health check enabled must be true for target groups with target type \
             '{target_type}'"
        );
        let message = crate::target_group_attributes_and_modification::unescaped(
            &reply.text("Message").unwrap_or_default(),
        );
        assert_eq!(
            message, expected,
            "case {target_type}: expected the message {expected:?}, got {message:?}"
        );
    }

    // A lambda target group may turn them on.
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", "lambda-hc-on"));
    form.push(p("TargetType", "lambda"));
    form.push(p("HealthCheckEnabled", "true"));
    let created = call(&router, &form).await;
    assert_eq!(
        created.status, 200,
        "case lambda with health checks on: expected 200, got {} body {}",
        created.status, created.body
    );
    let mut form = action("DescribeTargetGroups");
    form.push(p("Names.member.1", "lambda-hc-on"));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.text("HealthCheckEnabled").as_deref(),
        Some("true"),
        "case lambda with health checks on: expected the stored value to be true, got {}",
        reply.body
    );
}

// ---------------------------------------------------------------------------
// The store's own bounds.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_store_bound_is_answered_with_the_modelled_quota_error() {
    // Every store this service holds is bounded before the insert and answers the
    // operation's own modelled quota error past it (load balancers, target groups,
    // listeners per load balancer, targets per target group). The listener bound is
    // the cheapest to drive end to end, so it is the one exercised here.
    let router = router();
    let lb_arn = create_alb(&router, "quota-alb").await;
    let tg_arn = create_http_target_group(&router, "quota-tg").await;
    let bound = i32::try_from(LISTENERS_PER_LOAD_BALANCER).expect("the published bound");

    for port in 1..=bound {
        create_forward_listener(&router, &lb_arn, &tg_arn, port).await;
    }

    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", &lb_arn));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", &(bound + 1).to_string()));
    form.push(p("DefaultActions.member.1.Type", "forward"));
    form.push(p("DefaultActions.member.1.TargetGroupArn", &tg_arn));
    let refused = call(&router, &form).await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, "TooManyListeners"),
        "case one listener past the bound: expected 400 TooManyListeners, got {} {}",
        refused.status,
        refused.body
    );

    // The refused create stored nothing: the load balancer still carries exactly the
    // listeners that were accepted.
    let mut total = 0;
    let mut marker: Option<String> = None;
    loop {
        let mut form = action("DescribeListeners");
        form.push(p("LoadBalancerArn", &lb_arn));
        form.push(p("PageSize", "50"));
        if let Some(marker) = marker.as_deref() {
            form.push(p("Marker", marker));
        }
        let reply = call(&router, &form).await;
        total += members(&reply.body, "Listeners").len();
        marker = reply.text("NextMarker");
        if marker.is_none() {
            break;
        }
    }
    assert_eq!(
        total,
        usize::try_from(bound).expect("a positive bound"),
        "case listeners after the refusal: expected exactly the accepted listeners, got {total}"
    );

    // The target bound is refused for the WHOLE request, so a refusal stores none
    // of its targets.
    let target_bound = 1_000;
    let mut form = action("RegisterTargets");
    form.push(p("TargetGroupArn", &tg_arn));
    for index in 1..=target_bound {
        form.push(p(
            &format!("Targets.member.{index}.Id"),
            &format!("i-{index:017}"),
        ));
    }
    let registered = call(&router, &form).await;
    assert_eq!(
        registered.status, 200,
        "case registering exactly the bound: expected 200, got {} body {}",
        registered.status, registered.body
    );

    let mut form = action("RegisterTargets");
    form.push(p("TargetGroupArn", &tg_arn));
    // One target the group already holds (a replacement, which does not count) and
    // one new one, which does not fit.
    form.push(p("Targets.member.1.Id", "i-00000000000000001"));
    form.push(p("Targets.member.2.Id", "i-does-not-fit"));
    let refused = call(&router, &form).await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, "TooManyTargets"),
        "case one target past the bound: expected 400 TooManyTargets, got {} {}",
        refused.status,
        refused.body
    );

    let mut form = action("DescribeTargetHealth");
    form.push(p("TargetGroupArn", &tg_arn));
    let health = call(&router, &form).await;
    assert_eq!(
        members(&health.body, "TargetHealthDescriptions").len(),
        usize::try_from(target_bound).expect("a positive bound"),
        "case targets after the refusal: a refused request must store none of its targets"
    );
    let mut form = action("DescribeTargetHealth");
    form.push(p("TargetGroupArn", &tg_arn));
    form.push(p("Targets.member.1.Id", "i-does-not-fit"));
    let probe = call(&router, &form).await;
    assert_eq!(
        probe.text("Reason").as_deref(),
        Some("Target.NotRegistered"),
        "case the refused target: expected it to be unregistered, got {}",
        probe.body
    );
}

// ---------------------------------------------------------------------------
// Refusals for identifiers nothing in state carries.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unknown_identifier_is_refused_with_the_modelled_error() {
    let router = router();
    let lb_arn = create_alb(&router, "known-alb").await;
    let tg_arn = create_http_target_group(&router, "known-tg").await;
    let bogus_lb = format!(
        "arn:aws:elasticloadbalancing:{REGION}:{}:loadbalancer/app/nope/0123456789abcdef",
        account()
    );
    let bogus_tg = format!(
        "arn:aws:elasticloadbalancing:{REGION}:{}:targetgroup/nope/0123456789abcdef",
        account()
    );

    let cases: Vec<RefusalCase> = vec![
        (
            "create listener on an unknown load balancer",
            vec![
                p("Action", "CreateListener"),
                p("Version", VERSION),
                p("LoadBalancerArn", &bogus_lb),
                p("Protocol", "HTTP"),
                p("Port", "80"),
                p("DefaultActions.member.1.Type", "forward"),
                p("DefaultActions.member.1.TargetGroupArn", &tg_arn),
            ],
            400,
            "LoadBalancerNotFound",
        ),
        (
            "create listener forwarding to an unknown target group",
            vec![
                p("Action", "CreateListener"),
                p("Version", VERSION),
                p("LoadBalancerArn", &lb_arn),
                p("Protocol", "HTTP"),
                p("Port", "81"),
                p("DefaultActions.member.1.Type", "forward"),
                p("DefaultActions.member.1.TargetGroupArn", &bogus_tg),
            ],
            400,
            "TargetGroupNotFound",
        ),
        (
            "register targets with an unknown target group",
            vec![
                p("Action", "RegisterTargets"),
                p("Version", VERSION),
                p("TargetGroupArn", &bogus_tg),
                p("Targets.member.1.Id", "i-11111111111111111"),
            ],
            400,
            "TargetGroupNotFound",
        ),
        (
            "describe health of an unknown target group",
            vec![
                p("Action", "DescribeTargetHealth"),
                p("Version", VERSION),
                p("TargetGroupArn", &bogus_tg),
            ],
            400,
            "TargetGroupNotFound",
        ),
        (
            "describe load balancers by an unknown name",
            vec![
                p("Action", "DescribeLoadBalancers"),
                p("Version", VERSION),
                p("Names.member.1", "no-such-alb"),
            ],
            400,
            "LoadBalancerNotFound",
        ),
        (
            "describe load balancers by an unknown ARN",
            vec![
                p("Action", "DescribeLoadBalancers"),
                p("Version", VERSION),
                p("LoadBalancerArns.member.1", &bogus_lb),
            ],
            400,
            "LoadBalancerNotFound",
        ),
        (
            "describe target groups attached to an unknown load balancer",
            vec![
                p("Action", "DescribeTargetGroups"),
                p("Version", VERSION),
                p("LoadBalancerArn", &bogus_lb),
            ],
            400,
            "LoadBalancerNotFound",
        ),
        (
            // The model: "if the load balancer does not exist or has already been
            // deleted, the call succeeds."
            "delete a load balancer that is not there",
            vec![
                p("Action", "DeleteLoadBalancer"),
                p("Version", VERSION),
                p("LoadBalancerArn", &bogus_lb),
            ],
            200,
            "",
        ),
    ];

    for (case, form, status, code) in cases {
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, status,
            "case {case}: expected HTTP {status}, got {} body {}",
            reply.status, reply.body
        );
        if !code.is_empty() {
            assert_eq!(
                reply.error_code(),
                code,
                "case {case}: expected error code {code}, got {}",
                reply.body
            );
        }
    }
}
