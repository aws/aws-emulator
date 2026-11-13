// SPDX-License-Identifier: CC0-1.0

//! Converting a dedicated IP pool to another scaling mode, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised.
//!
//! The rules these cases pin, each over a SET of inputs rather than one recorded
//! request:
//!
//! 1. `PutDedicatedIpPoolScalingAttributes` WRITES the managed scaling mode onto the pool
//!    the request names, whatever mode that pool was in, and the pool's own read
//!    (`GetDedicatedIpPool`) then reports it; the conversion is idempotent, and it leaves
//!    everything else held on the pool — its name and its tags — and every other pool the
//!    account holds exactly as it was;
//! 2. a `ScalingMode` the conversion never applies is REFUSED `BadRequestException` (400)
//!    on the request alone — whether or not the account holds the pool it names — and
//!    changes nothing;
//! 3. a pool the account does not hold is refused `NotFoundException` (404), naming the
//!    pool the request asked about, and creates no pool.
//!
//! The tables span what those rules generalise over: the mode the pool starts in (both
//! the modes the enum declares, and a pool created without one), the pool's name form,
//! whether it carries tags, every leg of the parameter refusal (a held pool, a pool the
//! account never held, and values outside the modelled value set), and why the account
//! does not hold a name (never created, another case of a held name, a suffixed name, and
//! a pool deleted a moment ago).

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    SCALING_MODE_MANAGED, SCALING_MODE_STANDARD,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// `ListTagsForResource` binds `ResourceArn` to `@httpQuery`, and an ARN carries `:` and
// `/` — characters a query string gives its own meaning to — so the value is escaped the
// way an SDK escapes it, by the one escape this test binary shares.
use crate::contact_lists::query_escaped;

/// The account id every ARN this emulator mints carries, and the Region every call in
/// this suite is signed for, so the two an ARN it resolves has to carry.
const ACCOUNT_ID: &str = "000000000000";
const HOME: &str = "us-east-1";

/// Values outside `ScalingMode`'s modelled value set
/// (`smithy_query(query="shape",shape="ScalingMode")`: `STANDARD`, `MANAGED`): another
/// case of each declared value, a mode of some other API, a value with a declared one as
/// a prefix, and the empty value.
const UNMODELLED_SCALING_MODES: &[&str] = &["managed", "Standard", "AUTO", "MANAGED_POOL", ""];

/// `arn:aws:ses:<region>:<account>:dedicated-ip-pool/<name>`, the form SES writes a
/// dedicated IP pool of its own in.
fn pool_arn(pool_name: &str) -> String {
    format!("arn:aws:ses:{HOME}:{ACCOUNT_ID}:dedicated-ip-pool/{pool_name}")
}

/// One answered request: the status, the error shape when there was one, and the body.
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    ///
    /// restJson1 resolves the error shape from `x-amzn-errortype`; the JSON body's
    /// `__type` carries the same name, so either is read.
    fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The message the error carried, for an assertion that wants to show it.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A member reached by its path through the nested structures, or `Null`.
    fn at(&self, path: &[&str]) -> Value {
        let mut here = &self.body;
        for step in path {
            match here.get(step) {
                Some(next) => here = next,
                None => return Value::Null,
            }
        }
        here.clone()
    }

    /// A string at `path`, or `""` when the member is absent.
    fn text_at(&self, path: &[&str]) -> String {
        self.at(path).as_str().unwrap_or_default().to_string()
    }

    /// The strings of the list member at `path`; an absent list reads as empty.
    fn strings_at(&self, path: &[&str]) -> BTreeSet<String> {
        self.at(path)
            .as_array()
            .map(|rows| {
                rows.iter()
                    .map(|row| row.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The emulated service behind its router.
struct Ses {
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let host = format!("email.{HOME}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", "application/json")
            // The SigV4 credential scope is what routes the request to the `ses` signing
            // name; the signature itself is not checked here.
            .header(
                "authorization",
                format!("AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/{HOME}/ses/aws4_request, SignedHeaders=host, Signature=00"),
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    async fn create_pool(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/dedicated-ip-pools", Some(body))
            .await
    }

    async fn get_pool(&self, pool_name: &str) -> Reply {
        self.call(
            "GET",
            &format!("/v2/email/dedicated-ip-pools/{pool_name}"),
            None,
        )
        .await
    }

    async fn delete_pool(&self, pool_name: &str) -> Reply {
        self.call(
            "DELETE",
            &format!("/v2/email/dedicated-ip-pools/{pool_name}"),
            None,
        )
        .await
    }

    async fn list_pools(&self) -> Reply {
        self.call("GET", "/v2/email/dedicated-ip-pools", None).await
    }

    /// `PutDedicatedIpPoolScalingAttributes`'s route, from the operation's HTTP binding
    /// (`smithy_query(query="operation",operation="PutDedicatedIpPoolScalingAttributes")`:
    /// `PUT /v2/email/dedicated-ip-pools/{PoolName}/scaling`).
    async fn put_scaling(&self, pool_name: &str, scaling_mode: &str) -> Reply {
        self.call(
            "PUT",
            &format!("/v2/email/dedicated-ip-pools/{pool_name}/scaling"),
            Some(json!({"ScalingMode": scaling_mode})),
        )
        .await
    }

    /// `ListTagsForResource` for `resource_arn`, which the operation binds to
    /// `@httpQuery`.
    async fn list_tags(&self, resource_arn: &str) -> Reply {
        self.call(
            "GET",
            &format!("/v2/email/tags?ResourceArn={}", query_escaped(resource_arn)),
            None,
        )
        .await
    }
}

/// One pool of the tables below: how it is created, the mode it starts in, and the tags
/// the conversion has to leave alone.
struct Row {
    case: &'static str,
    pool_name: &'static str,
    body: Value,
    starting_mode: &'static str,
    tags: Vec<(&'static str, &'static str)>,
}

/// The pools every case of this suite works over.
///
/// The rows differ along the mode the pool starts in (a pool created without one, each
/// mode the enum declares), the name form (one character, both documented separators, a
/// long name) and whether the create carried tags.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a pool created with no ScalingMode",
            pool_name: "marketing-pool",
            body: json!({"PoolName": "marketing-pool"}),
            starting_mode: SCALING_MODE_STANDARD,
            tags: Vec::new(),
        },
        Row {
            case: "a STANDARD pool carrying tags",
            pool_name: "transactional_pool",
            body: json!({
                "PoolName": "transactional_pool",
                "ScalingMode": "STANDARD",
                "Tags": [{"Key": "team", "Value": "growth"}, {"Key": "tier", "Value": "gold"}],
            }),
            starting_mode: SCALING_MODE_STANDARD,
            tags: vec![("team", "growth"), ("tier", "gold")],
        },
        Row {
            case: "a pool already in the mode the conversion applies",
            pool_name: "managed-pool",
            body: json!({"PoolName": "managed-pool", "ScalingMode": "MANAGED"}),
            starting_mode: SCALING_MODE_MANAGED,
            tags: Vec::new(),
        },
        Row {
            case: "a one-character pool name",
            pool_name: "p",
            body: json!({"PoolName": "p"}),
            starting_mode: SCALING_MODE_STANDARD,
            tags: Vec::new(),
        },
        Row {
            case: "a long pool name carrying both documented separators",
            pool_name: "marketing_pool-for-eu-campaigns-2026",
            body: json!({
                "PoolName": "marketing_pool-for-eu-campaigns-2026",
                "ScalingMode": "STANDARD",
                "Tags": [{"Key": "owner", "Value": "eu"}],
            }),
            starting_mode: SCALING_MODE_STANDARD,
            tags: vec![("owner", "eu")],
        },
    ]
}

/// Create every pool of the table.
async fn seeded() -> (Ses, Vec<Row>) {
    let ses = Ses::new();
    let rows = rows();
    for row in &rows {
        let reply = ses.create_pool(row.body.clone()).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (200, ""),
            "case {:?}: expected 200 from the create that sets the case up, got {} {:?}",
            row.case,
            reply.status,
            reply.body
        );
    }
    (ses, rows)
}

/// The tag pairs a tag listing reported, in the order reported.
fn reported_tags(reply: &Reply) -> Vec<(String, String)> {
    reply
        .at(&["Tags"])
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    (
                        row["Key"].as_str().unwrap_or_default().to_string(),
                        row["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `ScalingMode` the pool's own read reports, with the status it answered.
async fn reported_mode(ses: &Ses, pool_name: &str) -> (u16, String) {
    let read = ses.get_pool(pool_name).await;
    (
        read.status,
        read.text_at(&["DedicatedIpPool", "ScalingMode"]),
    )
}

#[tokio::test]
async fn a_converted_pool_reports_the_managed_mode_and_nothing_else_changes() {
    let (ses, rows) = seeded().await;
    let mut converted: Vec<&Row> = Vec::new();
    let mut untouched: Vec<&Row> = rows.iter().collect();

    while !untouched.is_empty() {
        let row = untouched.remove(0);
        let case = row.case;

        let put = ses.put_scaling(row.pool_name, SCALING_MODE_MANAGED).await;
        assert_eq!(
            (put.status, put.error_code().as_str()),
            (200, ""),
            "case {case:?}: expected 200 from the conversion, got {} {:?}",
            put.status,
            put.body
        );

        // The round trip: the pool's own read reports the written mode, and the pool is
        // still the one the request named.
        let read = ses.get_pool(row.pool_name).await;
        assert_eq!(
            (
                read.status,
                read.text_at(&["DedicatedIpPool", "PoolName"]),
                read.text_at(&["DedicatedIpPool", "ScalingMode"]),
            ),
            (
                200,
                row.pool_name.to_string(),
                SCALING_MODE_MANAGED.to_string()
            ),
            "case {case:?}: expected the pool to report {:?}/{SCALING_MODE_MANAGED} after the conversion, got {} {:?}",
            row.pool_name,
            read.status,
            read.body
        );

        // Converting it again answers the same way and changes nothing further.
        let again = ses.put_scaling(row.pool_name, SCALING_MODE_MANAGED).await;
        assert_eq!(
            (again.status, again.error_code().as_str()),
            (200, ""),
            "case {case:?}: expected the repeated conversion to answer 200 too, got {} {:?}",
            again.status,
            again.body
        );
        assert_eq!(
            reported_mode(&ses, row.pool_name).await,
            (200, SCALING_MODE_MANAGED.to_string()),
            "case {case:?}: expected the repeated conversion to leave the mode alone"
        );

        // Everything else held on the pool survived the conversion.
        let tags = ses.list_tags(&pool_arn(row.pool_name)).await;
        let expected: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            reported_tags(&tags),
            expected,
            "case {case:?}: expected the conversion to leave the pool's tags alone, expected {expected:?}, got {:?}",
            reported_tags(&tags)
        );
        converted.push(row);

        // Every pool the conversion did not name is exactly where it was: the ones not
        // converted yet still report the mode they were created in, and the ones already
        // converted still report the converted mode.
        for other in &untouched {
            assert_eq!(
                reported_mode(&ses, other.pool_name).await,
                (200, other.starting_mode.to_string()),
                "case {case:?}: expected {:?} to still report the mode it was created in",
                other.pool_name
            );
        }
        for other in &converted {
            assert_eq!(
                reported_mode(&ses, other.pool_name).await,
                (200, SCALING_MODE_MANAGED.to_string()),
                "case {case:?}: expected {:?} to still report the converted mode",
                other.pool_name
            );
        }
    }

    // And the conversion created and removed no pool.
    let listed = ses.list_pools().await;
    let expected: BTreeSet<String> = rows.iter().map(|row| row.pool_name.to_string()).collect();
    assert_eq!(
        listed.strings_at(&["DedicatedIpPools"]),
        expected,
        "case \"the listing after every conversion\": expected exactly {expected:?}, got {:?}",
        listed.strings_at(&["DedicatedIpPools"])
    );
}

#[tokio::test]
async fn a_scaling_mode_the_conversion_never_applies_is_refused() {
    let (ses, rows) = seeded().await;
    // One pool starts in the mode the conversion applies, so a request that would move it
    // back is one of the rows below.
    let standard = rows[0].pool_name;
    let managed = rows[2].pool_name;

    // The rows differ along the pool the refused parameter was sent for: one in each mode
    // the enum declares, and a name the account holds no pool under at all — the mode is
    // judged on the request, so the pool it names does not change the answer.
    let cases = [
        ("the standard mode, for a STANDARD pool", standard),
        ("the standard mode, for a MANAGED pool", managed),
        ("the standard mode, for a pool the account does not hold", "no-such-pool"),
    ];

    for (case, pool_name) in cases {
        let reply = ses.put_scaling(pool_name, SCALING_MODE_STANDARD).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException, got {} {} {:?}",
            reply.status,
            reply.error_code(),
            reply.body
        );
        assert_eq!(
            reply.message(),
            "The ScalingMode parameter is invalid.",
            "case {case:?}: expected the parameter refusal, got {:?}",
            reply.message()
        );
    }

    // A value outside the modelled value set is refused as a bad request too, and no more
    // applied than the mode the conversion never applies.
    for value in UNMODELLED_SCALING_MODES {
        let reply = ses.put_scaling(managed, value).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {value:?}: expected 400 BadRequestException for a mode outside the value set, got {} {} {:?}",
            reply.status,
            reply.error_code(),
            reply.body
        );
    }

    // Every refusal left every pool in the mode it was in, and created no pool.
    for row in &rows {
        assert_eq!(
            reported_mode(&ses, row.pool_name).await,
            (200, row.starting_mode.to_string()),
            "case {:?}: expected the refusals to change nothing",
            row.case
        );
    }
    let listed = ses.list_pools().await;
    let expected: BTreeSet<String> = rows.iter().map(|row| row.pool_name.to_string()).collect();
    assert_eq!(
        listed.strings_at(&["DedicatedIpPools"]),
        expected,
        "case \"the listing after the refusals\": expected exactly {expected:?}, got {:?}",
        listed.strings_at(&["DedicatedIpPools"])
    );
}

#[tokio::test]
async fn converting_a_pool_the_account_does_not_hold_is_refused() {
    let (ses, rows) = seeded().await;
    // One pool is deleted, so "a pool the account held until a moment ago" is one of the
    // rows below.
    let gone = rows[0].pool_name;
    let deleted = ses.delete_pool(gone).await;
    assert_eq!(
        deleted.status, 200,
        "case \"the delete that sets a row up\": expected 200, got {} {:?}",
        deleted.status, deleted.body
    );
    let held: BTreeSet<String> = ses.list_pools().await.strings_at(&["DedicatedIpPools"]);

    // The rows differ along why the account does not hold the name: never created, another
    // case of a held name, a held name with a suffix, and a name it held until a moment
    // ago.
    let cases = [
        ("a pool that was never created", "no-such-pool".to_string()),
        (
            "another case of a held name",
            rows[1].pool_name.to_uppercase(),
        ),
        (
            "a held name with a suffix",
            format!("{}-2", rows[1].pool_name),
        ),
        ("a pool deleted a moment ago", gone.to_string()),
    ];

    for (case, pool_name) in &cases {
        let reply = ses.put_scaling(pool_name, SCALING_MODE_MANAGED).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException, got {} {} {:?}",
            reply.status,
            reply.error_code(),
            reply.body
        );
        // The refusal names the pool the request asked about, taken from the request
        // rather than from anything stored.
        assert_eq!(
            reply.message(),
            format!("The requested pool <{pool_name}> does not exist."),
            "case {case:?}: expected the refusal to name {pool_name:?}, got {:?}",
            reply.message()
        );
        // And the refused conversion created no pool under that name.
        let read = ses.get_pool(pool_name).await;
        assert_eq!(
            (read.status, read.error_code().as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected the refused conversion to create no pool, got {} {:?}",
            read.status,
            read.body
        );
    }

    let after: BTreeSet<String> = ses.list_pools().await.strings_at(&["DedicatedIpPools"]);
    assert_eq!(
        after, held,
        "case \"the listing after the refusals\": expected {held:?}, got {after:?}"
    );
}
