// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListLinks` as the enumeration of EVERY link the addressed
//! gateway holds, driven over the wire through the router the production emulator
//! uses.
//!
//! The rule under test is about state, not about one recorded request: the listing
//! reports every link record the store holds for the gateway in the URI — a link
//! between two gateways and a link with one end OUTSIDE the fabric alike, the kinds
//! the gateway's own `totalLinksCount` counts together — each entry rendered from its
//! stored record exactly as that record's own reader renders it, in `linkId` order, a
//! page at a time. So the page and the count one gateway reports agree, and a stored
//! link is reachable through the only listing this service has.
//!
//! The table spans what the rule generalises over: how many links between gateways
//! the gateway holds (none, one, two), how many links with one end outside the fabric
//! and in which direction (outbound on a requester gateway, inbound on an `EXTERNAL`
//! responder gateway), the `publicEndpoint` an outbound link carries (an `https` host,
//! an `http` host with a port and a path, a query string, and the longest address the
//! member's modelled length admits), whether such a link carries tags, and the page
//! size the walk takes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_rtbfabric_rtbfabric::ops::types::{
    CONNECTIVITY_TYPE_VALUES, LINK_DIRECTION_VALUES, LINK_STATUS_VALUES,
};
use service_com_amazonaws_rtbfabric_rtbfabric::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region the test signs for.
const REGION: &str = "us-west-2";

/// How long a reader waits for a status the clock owes. Generous: the suite waits for
/// the CONDITION, and the deadline only stops a hung loop.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long a reader waits between two looks at the same record.
const SETTLE_POLL: Duration = Duration::from_millis(20);

/// `LinkStatus` members, from `LINK_STATUS_VALUES` (this service's modelled
/// `LinkStatus`): the status the clock owes a link with one end outside the fabric,
/// and the one it owes a link whose request this service has taken.
const ACTIVE: &str = "ACTIVE";
const REQUESTED: &str = "REQUESTED";

/// `ConnectivityType` members, from `CONNECTIVITY_TYPE_VALUES`: both ends inside the
/// fabric, traffic leaving it, and traffic arriving from outside it.
const DEFAULT_CONNECTIVITY: &str = "DEFAULT";
const PUBLIC_EGRESS: &str = "PUBLIC_EGRESS";
const EXTERNAL_INBOUND: &str = "EXTERNAL_INBOUND";

/// `LinkDirection` members, from `LINK_DIRECTION_VALUES`: the direction of a link
/// opened on a requester gateway and of one created on a responder gateway.
const REQUEST: &str = "REQUEST";
const RESPONSE: &str = "RESPONSE";

/// The members a `ListLinks` entry and `GetLink` share, from
/// `smithy_query(query="shape", shape="ListLinksResponseStructure")` against
/// `GetLinkResponse`: the entry and that link's own reading describe ONE resource.
const SHARED_WITH_GET_LINK: &[&str] = &[
    "gatewayId",
    "peerGatewayId",
    "status",
    "createdAt",
    "updatedAt",
    "direction",
    "flowModules",
    "pendingFlowModules",
    "attributes",
    "logSettings",
    "connectivityType",
    "linkId",
    "tags",
];

/// The members a `ListLinks` entry and `GetOutboundExternalLink` share, from
/// `smithy_query(query="shape", shape="ListLinksResponseStructure")` against
/// `GetOutboundExternalLinkResponse` — `publicEndpoint` among them, which the entry
/// shape documents as "The public endpoint of the outbound link".
const SHARED_WITH_GET_OUTBOUND: &[&str] = &[
    "gatewayId",
    "status",
    "createdAt",
    "updatedAt",
    "flowModules",
    "pendingFlowModules",
    "attributes",
    "logSettings",
    "connectivityType",
    "linkId",
    "tags",
    "publicEndpoint",
];

/// The members a `ListLinks` entry and `GetInboundExternalLink` share, read the same
/// way. An inbound link's outside end is the `domainName` that reader reports, which
/// the entry shape has no member for.
const SHARED_WITH_GET_INBOUND: &[&str] = &[
    "gatewayId",
    "status",
    "createdAt",
    "updatedAt",
    "flowModules",
    "pendingFlowModules",
    "attributes",
    "logSettings",
    "connectivityType",
    "linkId",
    "tags",
];

/// The address an outbound external link carries. `URL` is `length(0..255)` with
/// pattern `^(https|http)://.+$` (`smithy_query(query="shape", shape="URL")`).
#[derive(Clone, Copy)]
enum Endpoint {
    Stated(&'static str),
    /// The longest address the member's modelled length admits.
    LongestAdmitted,
}

impl Endpoint {
    fn value(self) -> String {
        match self {
            Endpoint::Stated(url) => url.to_string(),
            Endpoint::LongestAdmitted => {
                let prefix = "https://bidder.example.com/";
                format!("{prefix}{}", "a".repeat(255 - prefix.len()))
            }
        }
    }
}

/// One outbound external link a row opens on its requester gateway.
struct Outbound {
    endpoint: Endpoint,
    tags: &'static [(&'static str, &'static str)],
}

/// One row of the table.
struct Row {
    name: &'static str,
    /// How many links between two gateways the requester gateway opens. The first of
    /// them goes to the row's `EXTERNAL` responder gateway, so that gateway's own
    /// listing mixes both kinds of link.
    peer_links: usize,
    /// The outbound external links the requester gateway opens.
    outbound: &'static [Outbound],
    /// How many inbound external links the `EXTERNAL` responder gateway takes.
    inbound: usize,
    /// The `maxResults` the paged walk states, or `None` for the listing's own page.
    max_results: Option<i64>,
}

/// The table. The rows differ along what the rule generalises over: the number of
/// links of each kind the gateway holds, the direction of the external ones, the
/// address an outbound link carries (including the longest the member admits),
/// whether it carries tags, and the page size.
const ROWS: &[Row] = &[
    Row {
        name: "one outbound external link and nothing else",
        peer_links: 0,
        outbound: &[Outbound {
            endpoint: Endpoint::Stated("https://bidder.example.com"),
            tags: &[],
        }],
        inbound: 0,
        max_results: None,
    },
    Row {
        name: "two links between gateways and two outbound external links, one per page",
        peer_links: 2,
        outbound: &[
            Outbound {
                endpoint: Endpoint::Stated("http://bid.example.net:8080/openrtb"),
                tags: &[("team", "rtb")],
            },
            Outbound {
                endpoint: Endpoint::LongestAdmitted,
                tags: &[("env", "test"), ("owner", "fabric")],
            },
        ],
        inbound: 0,
        max_results: Some(1),
    },
    Row {
        name: "a link between gateways, an outbound link, and three inbound external links",
        peer_links: 1,
        outbound: &[Outbound {
            endpoint: Endpoint::Stated("https://exchange.example.org/bid?seat=7"),
            tags: &[("owner", "fabric")],
        }],
        inbound: 3,
        max_results: Some(2),
    },
];

/// One answered call: the status and the parsed body.
struct Answer {
    status: u16,
    body: Value,
}

/// What kind of link a created record is, and what the listing must report for the
/// members that depend on the kind.
enum Kind {
    /// A link between two gateways, addressed from one of them.
    Peer { peer_gateway_id: String },
    /// A link whose traffic leaves the fabric for `endpoint`.
    Outbound { endpoint: String },
    /// A link whose traffic arrives from outside the fabric.
    Inbound,
}

/// One link this suite created, with the reader that answers about it.
struct Created {
    link_id: String,
    /// The gateway the listing under test is addressed to.
    gateway_id: String,
    /// The path that link's own single-record reader is addressed at.
    read_path: String,
    /// The status the clock owes the record.
    settled: &'static str,
    kind: Kind,
}

impl Created {
    fn peer(link_id: &str, gateway_id: &str, peer_gateway_id: &str) -> Self {
        Self {
            link_id: link_id.to_string(),
            gateway_id: gateway_id.to_string(),
            read_path: format!("/gateway/{gateway_id}/link/{link_id}"),
            settled: REQUESTED,
            kind: Kind::Peer {
                peer_gateway_id: peer_gateway_id.to_string(),
            },
        }
    }

    fn outbound(link_id: &str, gateway_id: &str, endpoint: &str) -> Self {
        Self {
            link_id: link_id.to_string(),
            gateway_id: gateway_id.to_string(),
            read_path: format!("/requester-gateway/{gateway_id}/outbound-external-link/{link_id}"),
            settled: ACTIVE,
            kind: Kind::Outbound {
                endpoint: endpoint.to_string(),
            },
        }
    }

    fn inbound(link_id: &str, gateway_id: &str) -> Self {
        Self {
            link_id: link_id.to_string(),
            gateway_id: gateway_id.to_string(),
            read_path: format!("/responder-gateway/{gateway_id}/inbound-external-link/{link_id}"),
            settled: ACTIVE,
            kind: Kind::Inbound,
        }
    }

    /// The members the entry and this record's own reader must agree on.
    fn shared_members(&self) -> &'static [&'static str] {
        match self.kind {
            Kind::Peer { .. } => SHARED_WITH_GET_LINK,
            Kind::Outbound { .. } => SHARED_WITH_GET_OUTBOUND,
            Kind::Inbound => SHARED_WITH_GET_INBOUND,
        }
    }

    /// The `peerGatewayId` the entry reports. A link with one end outside the fabric
    /// has no peer gateway in the fabric, so the only gateway it names is the one it
    /// belongs to; the member is `@required` and takes a `GatewayId`.
    fn peer_gateway_id(&self) -> &str {
        match &self.kind {
            Kind::Peer { peer_gateway_id } => peer_gateway_id.as_str(),
            Kind::Outbound { .. } | Kind::Inbound => self.gateway_id.as_str(),
        }
    }

    /// The `connectivityType` the entry reports, which is what tells the kinds apart.
    fn connectivity_type(&self) -> &'static str {
        match self.kind {
            Kind::Peer { .. } => DEFAULT_CONNECTIVITY,
            Kind::Outbound { .. } => PUBLIC_EGRESS,
            Kind::Inbound => EXTERNAL_INBOUND,
        }
    }

    /// The `publicEndpoint` the entry reports: the stored address of an outbound
    /// link, and nothing at all for any other kind.
    fn public_endpoint(&self) -> Value {
        match &self.kind {
            Kind::Outbound { endpoint } => json!(endpoint),
            Kind::Peer { .. } | Kind::Inbound => Value::Null,
        }
    }
}

/// The one emulated service, behind the same `MultiServiceRouter` the production
/// router assembles, so these calls take the real deserialize -> validate -> handle
/// -> serialize path.
struct Fabric {
    router: MultiServiceRouter,
}

impl Fabric {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Send one restJson1 request and return what came back.
    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> Answer {
        let bytes = match body {
            Some(body) => Bytes::from(serde_json::to_vec(&body).expect("serialize the body")),
            None => Bytes::new(),
        };
        let request = ::http::Request::builder()
            .method(method)
            .uri(format!("https://rtbfabric.{REGION}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/rtbfabric/aws4_request, \
                     SignedHeaders=host, Signature=00"
                ),
            )
            .body(bytes)
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Answer { status, body }
    }

    /// One answered call that must have succeeded.
    async fn ok(&self, case: &str, what: &str, method: &str, path: &str, body: Option<Value>) -> Value {
        let answered = self.send(method, path, body).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected {what} to answer 200, got {} {:?}",
            answered.status, answered.body
        );
        answered.body
    }

    /// A stored requester gateway, and its minted id.
    async fn requester_gateway(&self, case: &str) -> String {
        let body = self
            .ok(
                case,
                "CreateRequesterGateway",
                "POST",
                "/requester-gateway",
                Some(json!({
                    "vpcId": "vpc-0123456789abcdef0",
                    "subnetIds": ["subnet-0123456789abcdef0"],
                    "securityGroupIds": ["sg-0123456789abcdef0"],
                    "clientToken": format!("{case}-requester"),
                })),
            )
            .await;
        text(&body, "gatewayId")
    }

    /// A stored responder gateway, `EXTERNAL` or not, and its minted id.
    async fn responder_gateway(&self, case: &str, nth: usize, external: bool) -> String {
        let mut body = json!({
            "vpcId": "vpc-0123456789abcdef0",
            "subnetIds": ["subnet-0123456789abcdef0"],
            "securityGroupIds": ["sg-0123456789abcdef0"],
            "port": 443,
            "protocol": "HTTPS",
            "listenerConfig": { "protocols": ["HTTPS"] },
            "domainName": "responder.example.com",
            "clientToken": format!("{case}-responder-{nth}"),
        });
        if external {
            body["gatewayType"] = json!("EXTERNAL");
        }
        let answered = self
            .ok(
                case,
                "CreateResponderGateway",
                "POST",
                "/responder-gateway",
                Some(body),
            )
            .await;
        text(&answered, "gatewayId")
    }

    /// `POST /gateway/{gatewayId}/create-link`, and the minted link id.
    async fn create_link(&self, case: &str, gateway_id: &str, peer_gateway_id: &str) -> String {
        let body = self
            .ok(
                case,
                "CreateLink",
                "POST",
                &format!("/gateway/{gateway_id}/create-link"),
                Some(json!({
                    "peerGatewayId": peer_gateway_id,
                    "logSettings": log_settings(100.0, 0.0),
                })),
            )
            .await;
        text(&body, "linkId")
    }

    /// `POST /requester-gateway/{gatewayId}/outbound-external-link`, and the minted
    /// link id.
    async fn create_outbound(
        &self,
        case: &str,
        gateway_id: &str,
        nth: usize,
        link: &Outbound,
    ) -> String {
        let mut body = json!({
            "clientToken": format!("{case}-outbound-{nth}"),
            "publicEndpoint": link.endpoint.value(),
            "logSettings": log_settings(50.0, 25.0),
        });
        if !link.tags.is_empty() {
            body["tags"] = tags_json(link.tags);
        }
        let answered = self
            .ok(
                case,
                "CreateOutboundExternalLink",
                "POST",
                &format!("/requester-gateway/{gateway_id}/outbound-external-link"),
                Some(body),
            )
            .await;
        text(&answered, "linkId")
    }

    /// `POST /responder-gateway/{gatewayId}/inbound-external-link`, and the minted
    /// link id.
    async fn create_inbound(&self, case: &str, gateway_id: &str, nth: usize) -> String {
        let body = self
            .ok(
                case,
                "CreateInboundExternalLink",
                "POST",
                &format!("/responder-gateway/{gateway_id}/inbound-external-link"),
                Some(json!({
                    "clientToken": format!("{case}-inbound-{nth}"),
                    "logSettings": log_settings(0.0, 100.0),
                })),
            )
            .await;
        text(&body, "linkId")
    }

    /// Read `path` until it reports `expected` as its `status`, and answer with that
    /// reading.
    ///
    /// The suite waits for the CONDITION rather than for a span: it looks again until
    /// the status holds, giving up only once [`SETTLE_DEADLINE`] has passed.
    async fn poll_until_status(&self, case: &str, path: &str, expected: &str) -> Value {
        let deadline = Instant::now() + SETTLE_DEADLINE;
        loop {
            let body = self.ok(case, &format!("the read of {path:?}"), "GET", path, None).await;
            if text(&body, "status") == expected {
                return body;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected {path:?} to reach {expected:?}, got {:?}",
                body["status"]
            );
            tokio::time::sleep(SETTLE_POLL).await;
        }
    }

    /// One `ListLinks` call, as `(the entries, the nextToken)`.
    async fn list_links(
        &self,
        case: &str,
        gateway_id: &str,
        max_results: Option<i64>,
        token: Option<&str>,
    ) -> (Vec<Value>, Option<String>) {
        let mut query: Vec<String> = Vec::new();
        if let Some(max_results) = max_results {
            query.push(format!("maxResults={max_results}"));
        }
        if let Some(token) = token {
            query.push(format!("nextToken={token}"));
        }
        let path = match query.is_empty() {
            true => format!("/gateway/{gateway_id}/links/"),
            false => format!("/gateway/{gateway_id}/links/?{}", query.join("&")),
        };
        let body = self.ok(case, "ListLinks", "GET", &path, None).await;
        let entries = body["links"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected links to be a list for {path:?}, got {body:?}")
            })
            .clone();
        (entries, body["nextToken"].as_str().map(str::to_string))
    }

    /// Walk the whole listing of `gateway_id` at `max_results` per call, as the
    /// entries it reported in order.
    async fn walk_links(
        &self,
        case: &str,
        gateway_id: &str,
        max_results: Option<i64>,
    ) -> Vec<Value> {
        let mut walked: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let (entries, next) = self
                .list_links(case, gateway_id, max_results, token.as_deref())
                .await;
            if let Some(per_page) = max_results {
                assert!(
                    i64::try_from(entries.len()).unwrap_or(i64::MAX) <= per_page,
                    "case {case:?}: expected at most {per_page} entries per page, got {}",
                    entries.len()
                );
            }
            walked.extend(entries);
            pages += 1;
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
            assert!(
                pages <= 64,
                "case {case:?}: the walk of {gateway_id:?} did not terminate, {} entries so far",
                walked.len()
            );
        }
        walked
    }
}

fn text(body: &Value, member: &str) -> String {
    body[member].as_str().unwrap_or_default().to_string()
}

fn tags_json(tags: &[(&str, &str)]) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect(),
    )
}

/// `LinkLogSettings` with its one required member, as `smithy_query(query="shape",
/// shape="LinkLogSettings")` declares it; the percentages are a `Percentage`, whose
/// published range is 0..100.
fn log_settings(error_log: f64, filter_log: f64) -> Value {
    json!({ "applicationLogs": { "sampling": { "errorLog": error_log, "filterLog": filter_log } } })
}

/// The entry the listing reported for `link_id`, or a failure naming what it did
/// report.
fn entry_for<'entries>(case: &str, entries: &'entries [Value], link_id: &str) -> &'entries Value {
    entries
        .iter()
        .find(|entry| text(entry, "linkId") == link_id)
        .unwrap_or_else(|| {
            let listed: Vec<String> = entries.iter().map(|entry| text(entry, "linkId")).collect();
            panic!("case {case:?}: expected link {link_id:?} in the listing, got {listed:?}")
        })
}

/// Check one listed entry against the record's own reader and against what the kind
/// of link fixes.
async fn check_entry(fabric: &Fabric, case: &str, entry: &Value, created: &Created) {
    let read = fabric
        .ok(
            case,
            &format!("the read of {:?}", created.read_path),
            "GET",
            &created.read_path,
            None,
        )
        .await;
    for member in created.shared_members() {
        assert_eq!(
            entry[member], read[member],
            "case {case:?}: expected the listed entry and the reader of {:?} to agree on {member}, \
             got {:?} and {:?}",
            created.read_path, entry[member], read[member]
        );
    }
    assert_eq!(
        text(entry, "peerGatewayId"),
        created.peer_gateway_id(),
        "case {case:?}: expected the entry for {:?} to report peerGatewayId {:?}, got {:?}",
        created.link_id,
        created.peer_gateway_id(),
        entry["peerGatewayId"]
    );
    assert_eq!(
        text(entry, "connectivityType"),
        created.connectivity_type(),
        "case {case:?}: expected the entry for {:?} to report connectivityType {:?}, got {:?}",
        created.link_id,
        created.connectivity_type(),
        entry["connectivityType"]
    );
    assert_eq!(
        entry["publicEndpoint"],
        created.public_endpoint(),
        "case {case:?}: expected the entry for {:?} to report publicEndpoint {:?}, got {:?}",
        created.link_id,
        created.public_endpoint(),
        entry["publicEndpoint"]
    );
    assert_eq!(
        text(entry, "status"),
        created.settled,
        "case {case:?}: expected the entry for {:?} to report the settled status, got {:?}",
        created.link_id,
        entry["status"]
    );
    // Every reported enum member is one the model declares.
    assert!(
        LINK_STATUS_VALUES.contains(&text(entry, "status").as_str()),
        "case {case:?}: expected a modelled LinkStatus member, got {:?}",
        entry["status"]
    );
    assert!(
        CONNECTIVITY_TYPE_VALUES.contains(&text(entry, "connectivityType").as_str()),
        "case {case:?}: expected a modelled ConnectivityType member, got {:?}",
        entry["connectivityType"]
    );
    assert!(
        LINK_DIRECTION_VALUES.contains(&text(entry, "direction").as_str()),
        "case {case:?}: expected a modelled LinkDirection member, got {:?}",
        entry["direction"]
    );
    // The direction is a fact about which KIND of gateway the link was opened on: a
    // requester gateway opens requests, a responder gateway answers them.
    let expected_direction = match created.kind {
        Kind::Outbound { .. } => Some(REQUEST),
        Kind::Inbound => Some(RESPONSE),
        Kind::Peer { .. } => None,
    };
    if let Some(expected) = expected_direction {
        assert_eq!(
            text(entry, "direction"),
            expected,
            "case {case:?}: expected the entry for {:?} to report direction {expected:?}, got {:?}",
            created.link_id,
            entry["direction"]
        );
    }
}

/// Check the whole listing of one gateway against the links the row created on it.
async fn check_listing(
    fabric: &Fabric,
    case: &str,
    row: &Row,
    gateway_id: &str,
    gateway_read_path: &str,
    mut created: Vec<Created>,
) {
    created.sort_by(|left, right| left.link_id.cmp(&right.link_id));

    // Comparing a listing against a single-record reading is only meaningful once
    // nothing is owed, because a settle moves `status` and `updatedAt` together.
    for link in &created {
        fabric
            .poll_until_status(case, &link.read_path, link.settled)
            .await;
    }

    let listed = fabric.list_links(case, gateway_id, None, None).await.0;
    let listed_ids: Vec<String> = listed.iter().map(|entry| text(entry, "linkId")).collect();
    let expected_ids: Vec<String> = created.iter().map(|link| link.link_id.clone()).collect();
    assert_eq!(
        listed_ids, expected_ids,
        "case {case:?}: expected the listing of {gateway_id:?} to report every link it holds, \
         in linkId order, got {listed_ids:?}"
    );

    for link in &created {
        let entry = entry_for(case, &listed, &link.link_id);
        check_entry(fabric, case, entry, link).await;
    }

    // The paged walk reports the same enumeration, a page at a time.
    let walked = fabric.walk_links(case, gateway_id, row.max_results).await;
    let walked_ids: Vec<String> = walked.iter().map(|entry| text(entry, "linkId")).collect();
    assert_eq!(
        walked_ids, expected_ids,
        "case {case:?}: expected the paged walk of {gateway_id:?} at maxResults={:?} to report \
         every link once in order, got {walked_ids:?}",
        row.max_results
    );

    // A listing changes nothing a later caller sees: two of them at a settled store
    // are the same answer, entry for entry.
    let again = fabric.list_links(case, gateway_id, None, None).await.0;
    assert_eq!(
        again, listed,
        "case {case:?}: expected a repeated listing of {gateway_id:?} to report the same page, \
         got {again:?}"
    );

    // The gateway's own reader counts exactly the links the listing reports: one
    // gateway cannot answer two different things about the same records.
    let gateway = fabric
        .ok(
            case,
            &format!("the read of {gateway_read_path:?}"),
            "GET",
            gateway_read_path,
            None,
        )
        .await;
    assert_eq!(
        gateway["totalLinksCount"],
        json!(listed.len()),
        "case {case:?}: expected {gateway_id:?} to count the {} links its listing reports, got {:?}",
        listed.len(),
        gateway["totalLinksCount"]
    );
    let up = listed
        .iter()
        .filter(|entry| text(entry, "status") == ACTIVE)
        .count();
    assert_eq!(
        gateway["activeLinksCount"],
        json!(up),
        "case {case:?}: expected {gateway_id:?} to count the {up} listed links carrying traffic, \
         got {:?}",
        gateway["activeLinksCount"]
    );
}

#[tokio::test]
async fn the_listing_reports_every_link_the_gateway_holds() {
    for row in ROWS {
        let case = row.name;
        let fabric = Fabric::new();

        let requester = fabric.requester_gateway(case).await;
        // The `EXTERNAL` responder gateway: the one gateway type the
        // inbound-external-link surface exists for.
        let external_responder = fabric.responder_gateway(case, 0, true).await;

        let mut requester_links: Vec<Created> = Vec::new();
        let mut responder_links: Vec<Created> = Vec::new();

        for nth in 0..row.peer_links {
            let peer = match nth {
                0 => external_responder.clone(),
                _ => fabric.responder_gateway(case, nth, false).await,
            };
            let link_id = fabric.create_link(case, &requester, &peer).await;
            requester_links.push(Created::peer(&link_id, &requester, &peer));
            if peer == external_responder {
                // One record with two ends: the responder's listing reports it too,
                // addressed from the responder.
                responder_links.push(Created::peer(&link_id, &peer, &requester));
            }
        }
        for (nth, link) in row.outbound.iter().enumerate() {
            let link_id = fabric.create_outbound(case, &requester, nth, link).await;
            requester_links.push(Created::outbound(
                &link_id,
                &requester,
                &link.endpoint.value(),
            ));
        }
        for nth in 0..row.inbound {
            let link_id = fabric.create_inbound(case, &external_responder, nth).await;
            responder_links.push(Created::inbound(&link_id, &external_responder));
        }

        check_listing(
            &fabric,
            case,
            row,
            &requester,
            &format!("/requester-gateway/{requester}"),
            requester_links,
        )
        .await;
        check_listing(
            &fabric,
            case,
            row,
            &external_responder,
            &format!("/responder-gateway/{external_responder}"),
            responder_links,
        )
        .await;
    }
}
