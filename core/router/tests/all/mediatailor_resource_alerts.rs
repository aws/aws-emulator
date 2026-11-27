// SPDX-License-Identifier: CC0-1.0

//! The alerts MediaTailor holds against a resource, read end to end over HTTP.
//!
//! Every assertion here goes through the router: a signed request is built,
//! dispatched to the mounted MediaTailor service, and the JSON response read
//! back — so what is checked is the emulator's wire behaviour.
//!
//! The rule under test is the one the read implements: `ListAlerts` reports the
//! alerts the service HOLDS against the resource its `ResourceArn` addresses —
//! every stored alert of that resource and nothing of any other, member for
//! member, in one stable order, served in pages bounded by `MaxResults` with an
//! opaque continuation token — reporting the EMPTY set for a resource that
//! carries no alert, refusing an ARN the account holds no resource under as NOT
//! FOUND, and changing nothing in the store.
//!
//! The alerts are written straight into the one store the mounted service reads:
//! an alert is a condition MediaTailor raises against a resource, so the API
//! surface only reads them (`ListAlerts` is `@readonly` and the model declares no
//! alert write), and seeding the store is how this suite puts alerts in front of
//! the read. The emulator raises none itself, which is what the empty-set rows
//! assert.
//!
//! The tables span the class the rule generalises over: alerts filed under every
//! category the model declares and under none, with no related ARN, one and
//! several, seeded in an order that is not the reported one; both resource kinds
//! this service persists (a channel and a playback configuration); page sizes
//! from one to the published maximum and the size the request leaves unstated;
//! and ARNs addressing nothing — a deleted resource of either kind, a name never
//! created, another service's ARN and a string that is no ARN at all, each
//! refused in the words of the resource kind its ARN addressed.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mediatailor_mediatailor as mediatailor;
use service_com_amazonaws_mediatailor_mediatailor::state::{service_state, AlertRecord};

/// The Region this suite signs for, so the ARNs it builds are the ones the
/// request's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The account every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The signature a request of this suite carries. It is never verified here
/// (this router runs with request authentication off); the `Authorization`
/// header's CREDENTIAL SCOPE is what matters, since it routes the request to
/// this service and tells the handler which Region's ARN to mint.
const SIGNATURE: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// `ListAlerts`' success status: `GET /alerts` with `@http` code 200
/// (`smithy_query(query="operation",operation="ListAlerts")`).
const ALERTS_STATUS: u16 = 200;

/// The refusal every route of this service answers an ARN the account holds no
/// resource under with: HTTP 404 under the error code `NotFoundException`. The
/// sentence it carries names the resource the ARN ADDRESSED — a channel ARN is
/// refused in the words every channel route answers a channel miss with, a
/// playback-configuration ARN in that kind's words, which carry the
/// configuration's name — and an ARN naming no resource kind this service holds
/// gets the sentence the ARN-addressed routes answer a miss with.
const NOT_FOUND_STATUS: u16 = 404;
const NOT_FOUND_CODE: &str = "NotFoundException";
const RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource was not found";
const CHANNEL_NOT_FOUND_MESSAGE: &str = "The specified channel doesn't exist.";

/// The sentence a playback-configuration route answers a miss with, around the
/// `Name` the request addressed.
fn playback_configuration_not_found_message(name: &str) -> String {
    format!("PlaybackConfiguration {name} not found")
}

/// The refusal this service answers a request value its model cannot constrain
/// with: `BadRequestException`, HTTP 400 — the one client error the MediaTailor
/// model declares.
const BAD_REQUEST_STATUS: u16 = 400;
const BAD_REQUEST_CODE: &str = "BadRequestException";

/// `MaxResults`' modelled range on `ListAlerts` is 1..100
/// (`smithy_query(query="operation",operation="ListAlerts")`), and its
/// documentation gives 100 as the default, so 100 is both the largest page the
/// read serves and the page it serves when the request names no size.
const MAX_PAGE_SIZE: usize = 100;

/// One dispatched request's answer.
struct Reply {
    status: u16,
    /// The `x-amzn-errortype` header restJson1 resolves an error shape from.
    error_code: String,
    /// The parsed JSON body, or `Value::Null` for an empty one.
    body: Value,
}

impl Reply {
    fn get(&self, pointer: &str) -> &Value {
        self.body.pointer(pointer).unwrap_or(&Value::Null)
    }

    /// The refusal's text, read under whichever member the error shape that
    /// answered declares for it.
    ///
    /// The restJson1 codec keys a member by its `@jsonName` or else by the
    /// member name the model declares, so the member is a property of the error
    /// SHAPE: this service's one modelled error spells it `Message` and carries
    /// no `@jsonName`, while the shapes the service synthesises for the
    /// failures its model declares none for — its not-found envelope, the
    /// scaffold's not-implemented signal — spell it `message`. An error body
    /// carries exactly one of the two, so reading the one that is there reads
    /// the refusal's text whichever refused.
    fn message(&self) -> String {
        match self.get("/Message").as_str() {
            Some(text) => text.to_string(),
            None => self.get("/message").as_str().unwrap_or_default().to_string(),
        }
    }

    /// The `AlertCode` of each reported alert, in the order they were reported.
    fn reported_codes(&self) -> Vec<String> {
        self.get("/Items")
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        item.get("AlertCode")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The continuation token the reply carries, or `None` when it carries none.
    fn next_token(&self) -> Option<String> {
        self.get("/NextToken").as_str().map(str::to_string)
    }
}

/// The emulator under test: the MediaTailor service mounted on the real router.
struct Emulator {
    router: MultiServiceRouter,
    /// The SINGLE engine the mounted service reads and writes its state
    /// through, kept so this suite can seed the alerts no request writes.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(mediatailor::EmulatedService::new(engine.clone()));
        Self { router, engine }
    }

    async fn request(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        self.request_in(REGION, method, uri, body).await
    }

    /// Dispatch one request signed for `region`, which is what the handler reads
    /// the Region of an ARN it mints from.
    async fn request_in(
        &self,
        region: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> Reply {
        let payload = match body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("api.mediatailor.{region}.amazonaws.com"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{region}/mediatailor/\
                     aws4_request, SignedHeaders=host;x-amz-date, Signature={SIGNATURE}"
                ),
            )
            .header("content-type", "application/json")
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    /// `ListAlerts`: every member travels in the query string.
    async fn list_alerts(&self, resource_arn: &str, query: &[(&str, &str)]) -> Reply {
        let mut pairs = vec![format!("resourceArn={}", encode(resource_arn))];
        for (name, value) in query {
            pairs.push(format!("{name}={}", encode(value)));
        }
        self.request("GET", &format!("/alerts?{}", pairs.join("&")), None)
            .await
    }

    /// `CreateChannel`: `ChannelName` is the `@httpLabel`, every other member
    /// travels in the body. Answers the ARN the created channel reports.
    async fn create_channel(&self, name: &str) -> String {
        let created = self
            .request(
                "POST",
                &format!("/channel/{}", encode(name)),
                Some(json!({
                    "PlaybackMode": "LOOP",
                    "Outputs": [{ "ManifestName": "index", "SourceGroup": "default" }],
                })),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "setup: CreateChannel({name:?}) must succeed, got {} {:?}",
            created.status, created.body
        );
        created.get("/Arn").as_str().unwrap_or_default().to_string()
    }

    async fn delete_channel(&self, name: &str) {
        let deleted = self
            .request("DELETE", &format!("/channel/{}", encode(name)), None)
            .await;
        assert_eq!(
            deleted.status, 200,
            "setup: DeleteChannel({name:?}) must succeed, got {} {:?}",
            deleted.status, deleted.body
        );
    }

    /// `PutPlaybackConfiguration`: the configuration's `Name` travels in the
    /// body. Answers the ARN the stored configuration reports.
    async fn put_playback_configuration(&self, name: &str) -> String {
        self.put_playback_configuration_in(REGION, name).await
    }

    /// The same write, signed for `region`: the ARN a configuration reports
    /// carries the Region its request was signed for.
    async fn put_playback_configuration_in(&self, region: &str, name: &str) -> String {
        let put = self
            .request_in(
                region,
                "PUT",
                "/playbackConfiguration",
                Some(json!({
                    "Name": name,
                    "AdDecisionServerUrl": "https://ads.example.test/vast",
                    "VideoContentSourceUrl": "https://content.example.test/out",
                })),
            )
            .await;
        assert_eq!(
            put.status, 200,
            "setup: PutPlaybackConfiguration({name:?}) must succeed, got {} {:?}",
            put.status, put.body
        );
        put.get("/PlaybackConfigurationArn")
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    async fn delete_playback_configuration(&self, name: &str) {
        let deleted = self
            .request(
                "DELETE",
                &format!("/playbackConfiguration/{}", encode(name)),
                None,
            )
            .await;
        assert_eq!(
            deleted.status, 204,
            "setup: DeletePlaybackConfiguration({name:?}) must succeed, got {} {:?}",
            deleted.status, deleted.body
        );
    }

    /// Writes one alert into the one store the mounted service reads, held
    /// against `resource_arn` under its own `AlertCode`.
    fn seed_alert(&self, resource_arn: &str, seed: &Seed) {
        let state = service_state(&*self.engine).expect("the service's own store");
        let mut store = state.lock();
        store
            .alerts
            .entry(resource_arn.to_string())
            .or_default()
            .insert(seed.code.to_string(), seed.record(resource_arn));
    }

    /// How many alerts the store holds against `resource_arn`, read from the
    /// store itself — so a row can assert that a READ left it alone.
    fn held_alert_codes(&self, resource_arn: &str) -> Vec<String> {
        let state = service_state(&*self.engine).expect("the service's own store");
        let store = state.lock();
        store
            .alerts
            .get(resource_arn)
            .map(|held| held.keys().cloned().collect())
            .unwrap_or_default()
    }
}

/// Percent-encode a value bound to a URI path segment or query parameter, the
/// way an AWS SDK does: everything outside the RFC 3986 unreserved set is
/// escaped, so an ARN's `:` and `/` travel as one value.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// The ARN a channel of this emulator reports, which is what addresses it in an
/// alerts read: `arn:aws:mediatailor:<region>:<account>:channel/<name>`.
fn channel_arn(name: &str) -> String {
    format!("arn:aws:mediatailor:{REGION}:{ACCOUNT}:channel/{name}")
}

/// One alert a row seeds. The members are `Alert`'s own
/// (`smithy_query(query="shape",shape="Alert")`): `AlertCode` and
/// `AlertMessage` are plain strings the model constrains to no value set — the
/// member's documentation gives `NOT_PROCESSED` as an example code, and the
/// other codes here are this suite's own data — while `Category` is the one
/// optional member and carries a value of `AlertCategory`.
struct Seed {
    code: &'static str,
    message: &'static str,
    last_modified_secs: i64,
    /// The related ARNs the alert carries: none, the resource's own, or several.
    related: &'static [&'static str],
    category: Option<&'static str>,
}

impl Seed {
    fn record(&self, resource_arn: &str) -> AlertRecord {
        AlertRecord {
            alert_code: self.code.to_string(),
            alert_message: self.message.to_string(),
            last_modified_time: ::aws_smithy_types::DateTime::from_secs(self.last_modified_secs),
            related_resource_arns: self.related.iter().map(|arn| (*arn).to_string()).collect(),
            resource_arn: resource_arn.to_string(),
            category: self.category.map(str::to_string),
        }
    }

    /// The alert as the wire reports it, against the resource it is held
    /// against.
    fn expected(&self, resource_arn: &str) -> Value {
        let mut expected = json!({
            "AlertCode": self.code,
            "AlertMessage": self.message,
            "LastModifiedTime": self.last_modified_secs,
            "RelatedResourceArns": self.related,
            "ResourceArn": resource_arn,
        });
        if let Some(category) = self.category {
            expected
                .as_object_mut()
                .expect("the expected alert is a JSON object")
                .insert("Category".to_string(), json!(category));
        }
        expected
    }
}

/// `AlertCategory`'s modelled values
/// (`smithy_query(query="shape",shape="AlertCategory")` reports
/// `SCHEDULING_ERROR`, `PLAYBACK_WARNING` and `INFO`), so the table below files
/// an alert under each of them and one under none.
const SCHEDULING_ERROR: &str = "SCHEDULING_ERROR";
const PLAYBACK_WARNING: &str = "PLAYBACK_WARNING";
const INFO: &str = "INFO";

/// A fixed instant the rows date their alerts around, in epoch seconds, so what
/// a read reports depends on no clock.
const BASE_SECONDS: i64 = 1_700_000_000;

/// The alerts the projection table spans, in an order that is NOT the one a read
/// reports them in: every category and none, no related ARN, one and several.
const PROJECTION_SEEDS: &[Seed] = &[
    Seed {
        code: "SOURCE_UNREACHABLE",
        message: "The source location could not be reached.",
        last_modified_secs: BASE_SECONDS + 30,
        related: &[],
        category: Some(PLAYBACK_WARNING),
    },
    Seed {
        code: "NOT_PROCESSED",
        message: "The request was not processed.",
        last_modified_secs: BASE_SECONDS,
        related: &["arn:aws:mediatailor:us-east-1:000000000000:channel/related-one"],
        category: Some(SCHEDULING_ERROR),
    },
    Seed {
        code: "ZONE_OFFSET_APPLIED",
        message: "A time-shift offset is in effect for this resource.",
        last_modified_secs: BASE_SECONDS + 7,
        related: &[
            "arn:aws:mediatailor:us-east-1:000000000000:channel/related-one",
            "arn:aws:mediatailor:us-east-1:000000000000:channel/related-two",
        ],
        category: Some(INFO),
    },
    Seed {
        code: "AD_BREAK_SKIPPED",
        message: "An ad break was skipped.",
        last_modified_secs: BASE_SECONDS + 900,
        related: &[],
        category: None,
    },
];

/// Two more conditions, so the pagination rows have a set that does not divide
/// evenly by every page size they ask for.
const EXTRA_SEEDS: &[Seed] = &[
    Seed {
        code: "MANIFEST_WINDOW_SHORT",
        message: "The manifest window is shorter than the program it carries.",
        last_modified_secs: BASE_SECONDS + 11,
        related: &[],
        category: Some(PLAYBACK_WARNING),
    },
    Seed {
        code: "SLATE_MISSING",
        message: "The filler slate could not be read.",
        last_modified_secs: BASE_SECONDS + 12,
        related: &[],
        category: Some(SCHEDULING_ERROR),
    },
];

/// The codes of `seeds`, in the order a read has to report them: ascending by
/// `AlertCode`, the identifier an alert of one resource is held under.
fn expected_order(seeds: &[&Seed]) -> Vec<String> {
    let mut codes: Vec<String> = seeds.iter().map(|seed| seed.code.to_string()).collect();
    codes.sort();
    codes
}

/// The two resource kinds this service persists, each created on `emulator` and
/// answered as `(case, name, arn)`.
async fn resources(emulator: &Emulator) -> Vec<(&'static str, &'static str, String)> {
    let channel = emulator.create_channel("alerts-channel").await;
    let configuration = emulator
        .put_playback_configuration("alerts-configuration")
        .await;
    vec![
        ("a channel", "alerts-channel", channel),
        (
            "a playback configuration",
            "alerts-configuration",
            configuration,
        ),
    ]
}

#[tokio::test]
async fn a_resources_alerts_are_reported_member_for_member_in_one_order() {
    let emulator = Emulator::new();

    for (case, _, arn) in resources(&emulator).await {
        for seed in PROJECTION_SEEDS {
            emulator.seed_alert(&arn, seed);
        }

        let listed = emulator.list_alerts(&arn, &[]).await;
        assert_eq!(
            listed.status, ALERTS_STATUS,
            "case {case:?}: expected the alerts of a stored resource to be served, got {} {:?}",
            listed.status, listed.body
        );

        let expected: Vec<Value> = {
            let mut ordered: Vec<&Seed> = PROJECTION_SEEDS.iter().collect();
            ordered.sort_by_key(|seed| seed.code);
            ordered.iter().map(|seed| seed.expected(&arn)).collect()
        };
        assert_eq!(
            listed.get("/Items"),
            &json!(expected),
            "case {case:?}: expected every stored alert of the resource, member for member and \
             ordered by code, got {:?}",
            listed.get("/Items")
        );
        assert_eq!(
            listed.next_token(),
            None,
            "case {case:?}: expected one page to carry every alert of the resource, got the token \
             {:?}",
            listed.next_token()
        );

        // The read reports the store; it does not change it, and reading again
        // answers the same.
        let again = emulator.list_alerts(&arn, &[]).await;
        assert_eq!(
            again.body, listed.body,
            "case {case:?}: expected a second read to answer the same alerts, got {:?}",
            again.body
        );
        assert_eq!(
            emulator.held_alert_codes(&arn),
            expected_order(&PROJECTION_SEEDS.iter().collect::<Vec<_>>()),
            "case {case:?}: expected the read to leave the stored alerts alone, got {:?}",
            emulator.held_alert_codes(&arn)
        );
    }
}

#[tokio::test]
async fn a_resource_with_no_alert_reports_the_empty_set() {
    let emulator = Emulator::new();
    let listed_resources = resources(&emulator).await;

    // Nothing is seeded, so every resource carries no alert — the answer this
    // emulator gives every resource it mints, since nothing it runs raises one.
    for (case, _, arn) in &listed_resources {
        let listed = emulator.list_alerts(arn, &[]).await;
        assert_eq!(
            listed.status, ALERTS_STATUS,
            "case {case:?}: expected the read to be served, got {} {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.get("/Items"),
            &json!([]),
            "case {case:?}: expected a resource with no alert to report the empty set, got {:?}",
            listed.get("/Items")
        );
        assert_eq!(
            listed.next_token(),
            None,
            "case {case:?}: expected no continuation token for an empty set, got {:?}",
            listed.next_token()
        );
    }

    // An alert held against ONE resource is reported for that resource only:
    // the other still reports the empty set.
    let (holder_case, _, holder_arn) = &listed_resources[0];
    let (other_case, _, other_arn) = &listed_resources[1];
    emulator.seed_alert(holder_arn, &PROJECTION_SEEDS[0]);
    assert_eq!(
        emulator.list_alerts(holder_arn, &[]).await.reported_codes(),
        vec![PROJECTION_SEEDS[0].code.to_string()],
        "case {holder_case:?}: expected the resource the alert is held against to report it"
    );
    assert_eq!(
        emulator.list_alerts(other_arn, &[]).await.get("/Items"),
        &json!([]),
        "case {other_case:?}: expected a resource holding no alert to report none of another \
         resource's"
    );
}

#[tokio::test]
async fn alerts_are_served_in_pages_bounded_by_max_results() {
    let emulator = Emulator::new();
    let arn = emulator.create_channel("paged-alerts").await;

    let seeds: Vec<&Seed> = PROJECTION_SEEDS.iter().chain(EXTRA_SEEDS).collect();
    for seed in &seeds {
        emulator.seed_alert(&arn, seed);
    }
    let expected = expected_order(&seeds);

    // Page sizes from one to the published maximum, and the size a request that
    // names none is served: every walk reports every alert, in one order, each
    // exactly once, and only the LAST page of a walk carries no token.
    for page_size in [None, Some(1), Some(2), Some(3), Some(seeds.len()), Some(MAX_PAGE_SIZE)] {
        let case = format!("maxResults={page_size:?}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;

        loop {
            let mut query: Vec<(&str, &str)> = Vec::new();
            let size = page_size.map(|size| size.to_string());
            if let Some(size) = size.as_deref() {
                query.push(("maxResults", size));
            }
            if let Some(token) = token.as_deref() {
                query.push(("nextToken", token));
            }
            let listed = emulator.list_alerts(&arn, &query).await;
            assert_eq!(
                listed.status, ALERTS_STATUS,
                "case {case:?}: expected page {pages} to be served, got {} {:?}",
                listed.status, listed.body
            );

            let codes = listed.reported_codes();
            assert!(
                codes.len() <= page_size.unwrap_or(MAX_PAGE_SIZE),
                "case {case:?}: expected page {pages} to hold at most the page size, got {codes:?}"
            );
            walked.extend(codes);
            pages += 1;
            assert!(
                pages <= seeds.len() + 1,
                "case {case:?}: expected the walk to end, got {pages} pages and {walked:?}"
            );

            match listed.next_token() {
                Some(next) => token = Some(next),
                None => break,
            }
        }

        assert_eq!(
            walked, expected,
            "case {case:?}: expected the walk to report every alert once, in order"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    let arn = emulator.create_channel("token-alerts").await;
    for seed in PROJECTION_SEEDS {
        emulator.seed_alert(&arn, seed);
    }

    // A token is opaque, so the only thing a caller can send that this service
    // did not issue is a value its codec cannot read: not hex at all, hex of an
    // odd length, or hex of bytes that are no text.
    for token in ["not-a-token", "zzzz", "abc", "ff"] {
        let refused = emulator.list_alerts(&arn, &[("nextToken", token)]).await;
        assert_eq!(
            refused.status, BAD_REQUEST_STATUS,
            "case {token:?}: expected a token this service never issued to be refused, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code, BAD_REQUEST_CODE,
            "case {token:?}: expected the refusal to answer {BAD_REQUEST_CODE}, got {:?}",
            refused.error_code
        );
        assert!(
            refused.message().contains("NextToken"),
            "case {token:?}: expected the refusal to name the member, got {:?}",
            refused.message()
        );
    }

    // And the store is untouched by every refusal: the alerts are still there to
    // be read.
    assert_eq!(
        emulator.list_alerts(&arn, &[]).await.reported_codes(),
        expected_order(&PROJECTION_SEEDS.iter().collect::<Vec<_>>()),
        "expected the refused reads to leave the resource's alerts readable"
    );
}

#[tokio::test]
async fn an_arn_that_addresses_no_resource_is_refused_as_not_found() {
    let emulator = Emulator::new();

    // A resource of each kind, created, carrying an alert, then DELETED: its
    // alerts go with it, so its ARN addresses nothing to read them from.
    let deleted_channel = emulator.create_channel("deleted-channel").await;
    emulator.seed_alert(&deleted_channel, &PROJECTION_SEEDS[0]);
    emulator.delete_channel("deleted-channel").await;

    let configuration_arn = emulator
        .put_playback_configuration("deleted-configuration")
        .await;
    emulator.seed_alert(&configuration_arn, &PROJECTION_SEEDS[1]);
    emulator
        .delete_playback_configuration("deleted-configuration")
        .await;

    let rows: Vec<(&str, String, String)> = vec![
        (
            "a deleted channel's ARN",
            deleted_channel.clone(),
            CHANNEL_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "a deleted playback configuration's ARN",
            configuration_arn.clone(),
            playback_configuration_not_found_message("deleted-configuration"),
        ),
        (
            "a channel name never created",
            channel_arn("absent"),
            CHANNEL_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "a resource type this service does not persist",
            format!("arn:aws:mediatailor:{REGION}:{ACCOUNT}:sourceLocation/absent"),
            RESOURCE_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "another service's ARN",
            format!("arn:aws:medialive:{REGION}:{ACCOUNT}:channel/absent"),
            RESOURCE_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "a string that is no ARN",
            "not-an-arn".to_string(),
            RESOURCE_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "an ARN of another account",
            format!("arn:aws:mediatailor:{REGION}:111122223333:channel/deleted-channel"),
            CHANNEL_NOT_FOUND_MESSAGE.to_string(),
        ),
    ];

    for (case, arn, message) in rows {
        let refused = emulator.list_alerts(&arn, &[]).await;
        assert_eq!(
            refused.status, NOT_FOUND_STATUS,
            "case {case:?}: expected the read to be refused as not found, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code, NOT_FOUND_CODE,
            "case {case:?}: expected the refusal to answer {NOT_FOUND_CODE}, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            message,
            "case {case:?}: expected the refusal to answer {message:?}, got {:?}",
            refused.message()
        );
    }

    // The deletes released what was held against those ARNs, so a resource
    // created again under the same name carries none of the deleted one's
    // alerts.
    let recreated = emulator.create_channel("deleted-channel").await;
    assert_eq!(
        recreated, deleted_channel,
        "expected the recreated channel to report the same ARN, got {recreated:?}"
    );
    let listed = emulator.list_alerts(&recreated, &[]).await;
    assert_eq!(
        listed.get("/Items"),
        &json!([]),
        "expected the new channel to carry no alert of the deleted one, got {:?}",
        listed.get("/Items")
    );
    assert_eq!(
        emulator.held_alert_codes(&recreated),
        Vec::<String>::new(),
        "expected the delete to have released the alerts held against the ARN"
    );
}

#[tokio::test]
async fn a_replaced_configurations_alerts_follow_the_arn_it_reports() {
    let emulator = Emulator::new();
    let first = emulator
        .put_playback_configuration("replaced-configuration")
        .await;
    emulator.seed_alert(&first, &PROJECTION_SEEDS[0]);

    // A Put over a stored name REPLACES the configuration, and the ARN it
    // reports carries the Region the request was signed for, so a replacement
    // signed for another Region re-mints it. An alert is held against the
    // RESOURCE, not against the string that addresses it, so it is read through
    // the ARN the resource now reports.
    let replaced = emulator
        .put_playback_configuration_in("eu-west-1", "replaced-configuration")
        .await;
    assert_ne!(
        replaced, first,
        "the replacement must re-mint the ARN for this row to be about the move"
    );

    let listed = emulator.list_alerts(&replaced, &[]).await;
    assert_eq!(
        listed.status, ALERTS_STATUS,
        "expected the replaced configuration's alerts to be served, got {} {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.get("/Items"),
        &json!([PROJECTION_SEEDS[0].expected(&replaced)]),
        "expected the alert held against the configuration to survive its replacement and report \
         the ARN the resource now reports, got {:?}",
        listed.get("/Items")
    );

    // The ARN it reported before addresses no resource now, and nothing is left
    // held against it.
    let refused = emulator.list_alerts(&first, &[]).await;
    assert_eq!(
        refused.status, NOT_FOUND_STATUS,
        "expected the ARN no resource reports to address nothing, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.message(),
        playback_configuration_not_found_message("replaced-configuration"),
        "expected the refusal to name the configuration the ARN addressed, got {:?}",
        refused.message()
    );
    let stray: BTreeMap<String, usize> = {
        let state = service_state(&*emulator.engine).expect("the service's own store");
        let store = state.lock();
        store
            .alerts
            .iter()
            .filter(|(held_arn, _)| held_arn.as_str() != replaced.as_str())
            .map(|(held_arn, held)| (held_arn.clone(), held.len()))
            .collect()
    };
    assert!(
        stray.is_empty(),
        "expected no alert to be held against an ARN no resource reports, got {stray:?}"
    );
}
