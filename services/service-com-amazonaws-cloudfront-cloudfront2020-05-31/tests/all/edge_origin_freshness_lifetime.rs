// SPDX-License-Identifier: CC0-1.0

//! WHICH span the edge holds an origin answer for, end to end over HTTP, with Amazon S3 mounted
//! beside CloudFront on one engine so every directive under test is one an origin really stated
//! about a real object.
//!
//! RULE: an answer the origin admits to a shared cache is held for the FRESHNESS LIFETIME the
//! origin stated for it — `Cache-Control`'s `s-maxage`, the directive addressed to a shared
//! cache, else its `max-age`, else the instant `Expires` carries — bounded by the behaviour's own
//! spans: never below `MinTTL`, "the minimum amount of time ... that you want objects to stay in
//! the CloudFront cache before CloudFront sends another request to the origin", and never above
//! `MaxTTL`, "the maximum amount of time ... that objects stay in the CloudFront cache", which
//! CloudFront "uses ... only when the origin sends `Cache-Control` or `Expires` headers with the
//! object". `DefaultTTL` is the span for an answer whose origin stated NO lifetime, and only for
//! one: CloudFront "uses this value as the object's time to live (TTL) only when the origin does
//! *not* send `Cache-Control` or `Expires` headers with the object"
//! (`smithy_query(query="doc",shape="CachePolicyConfig",member="MinTTL"|"MaxTTL"|"DefaultTTL")`).
//!
//! The rows differ along what the rule generalises over: the directive the origin stated (none,
//! `max-age`, `s-maxage` beside a `max-age`, a directive that forbids storage, an argument that
//! is no span at all), where the stated span falls against the behaviour's bounds (inside them,
//! under the minimum, no time at all, over the maximum), and WHERE the behaviour's three spans
//! come from — a cache policy the account created, or the legacy per-behaviour members — since
//! the rule is the same under both.
//!
//! The span itself is read off the entry the edge holds, so no case waits a TTL out; that the
//! span is really what makes the edge ask the origin again is the second test's, which replaces
//! the object at the origin and polls for the refreshed bytes under a short stated lifetime while
//! an object held under the default span keeps serving the copy the edge already has.
//!
//! The harness is the siblings' own: the two services over one engine from
//! [`crate::edge_signed_s3_origin`], the origin access control, the distribution over a bucket
//! and the viewer client from [`crate::edge_origin_response_headers`], and the cache policy
//! document from [`crate::cache_policy_lifecycle`] — reused rather than copied.

use std::sync::Arc;
use std::time::{Duration, Instant};

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::ComAmazonawsCloudfrontCloudfront20200531State;
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use crate::cache_policy_lifecycle::{create_policy, Policy};
use crate::distribution_lifecycle::{awaited_deployment, POLL_INTERVAL, READY_DEADLINE};
use crate::edge_origin_response_headers::{
    create_distribution_over_bucket, create_origin_access_control, view, HIT, MISS, REGION,
};
use crate::edge_signed_s3_origin::{grant, mounted};

/// The three spans every row below is read under. They are three DIFFERENT values, so a row's
/// expected span says which of them (or the origin's own) decided it: `MinTTL` floors a shorter
/// lifetime, `MaxTTL` caps a longer one, and `DefaultTTL` is reached only where the origin stated
/// none.
const MIN_TTL: u64 = 12;
const DEFAULT_TTL: u64 = 60;
const MAX_TTL: u64 = 900;

/// The bytes the object carries when it is first staged, and the ones that replace them when a
/// case asks whether the edge went back to the origin.
const FIRST: &str = "content-v1";
const REFRESHED: &str = "content-v2";

/// CloudFront and the object store its origin is, over ONE engine this suite KEEPS: the span an
/// entry is held for is read off the store the edge holds that entry in.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    callback: EmbeddedEmulator,
}

impl Emulator {
    /// Both services on one router over one engine, through the shared constructor.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = mounted(&engine, false);
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self {
            router,
            engine,
            callback,
        }
    }

    /// The Amazon S3 client, built by the one shared constructor over the in-process callback, so
    /// the bucket this suite stages is the one the edge's origin fetch reads.
    fn s3(&self) -> ::aws_sdk_s3::Client {
        service_common::s3_client::s3_client(&self.callback, REGION)
    }

    /// Create `bucket`. It carries no public access: what reaches an object is the grant the
    /// distribution's origin access control is given.
    async fn staged_bucket(&self, case: &str, bucket: &str) {
        self.s3()
            .create_bucket()
            .bucket(bucket)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: creating {bucket} must succeed, got {error:?}")
            });
    }

    /// Put `body` at `key` in `bucket`, under the `Cache-Control` the origin states about it (or
    /// none at all).
    async fn staged_object(
        &self,
        case: &str,
        bucket: &str,
        key: &str,
        body: &str,
        cache_control: Option<&str>,
    ) {
        let mut put = self
            .s3()
            .put_object()
            .bucket(bucket)
            .key(key)
            .body(::aws_sdk_s3::primitives::ByteStream::from(
                body.as_bytes().to_vec(),
            ))
            .content_type("text/plain");
        if let Some(stated) = cache_control {
            put = put.cache_control(stated);
        }
        put.send().await.unwrap_or_else(|error| {
            panic!("case {case:?}: putting {bucket}/{key} must succeed, got {error:?}")
        });
    }

    /// Grant the CloudFront service principal the read of `bucket`'s objects for the one
    /// distribution `arn` names, which is the document AWS publishes for an origin access
    /// control.
    async fn granted(&self, case: &str, bucket: &str, arn: &str) {
        self.s3()
            .put_bucket_policy()
            .bucket(bucket)
            .policy(grant(bucket, arn))
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: the policy of {bucket} must be stored, got {error:?}")
            });
    }

    /// The span the edge cache holds the entry for `path` under `distribution_id` for — its own
    /// expiry less the instant it was filled — or `None` when the cache holds no entry for it.
    fn held_span(&self, case: &str, distribution_id: &str, path: &str) -> Option<Duration> {
        let state = self
            .engine
            .state::<ComAmazonawsCloudfrontCloudfront20200531State>();
        let store = state.lock();
        let distribution = store.distributions.get(distribution_id).unwrap_or_else(|| {
            panic!("case {case:?}: the store holds distribution {distribution_id}")
        });
        distribution
            .edge_cache
            .values()
            .find(|entry| entry.path == path)
            .map(|entry| {
                entry
                    .expires_at
                    .duration_since(entry.stored_at)
                    .unwrap_or_default()
            })
    }
}

/// WHERE a behaviour's three spans come from: both places the service reads them.
#[derive(Debug, Clone, Copy)]
enum Spans {
    /// A cache policy the account created, which the behaviour names.
    CachePolicy,
    /// The legacy per-behaviour `MinTTL`/`DefaultTTL`/`MaxTTL`, beside `ForwardedValues`.
    Behavior,
}

impl Spans {
    /// What this suite calls the configuration, which names its bucket and caller reference too.
    fn name(self) -> &'static str {
        match self {
            Self::CachePolicy => "edge-freshness-cache-policy",
            Self::Behavior => "edge-freshness-behavior",
        }
    }
}

/// The `DefaultCacheBehavior` members that state the three spans, from `spans`.
async fn behavior_spans(emulator: &Emulator, spans: Spans) -> String {
    match spans {
        Spans::CachePolicy => {
            let created = create_policy(
                &emulator.router,
                &Policy {
                    case: "the spans every row is read under",
                    name: spans.name().to_string(),
                    comment: None,
                    min_ttl: i64::try_from(MIN_TTL).expect("the minimum span is a count"),
                    default_ttl: Some(i64::try_from(DEFAULT_TTL).expect("a count")),
                    max_ttl: Some(i64::try_from(MAX_TTL).expect("a count")),
                    parameters: None,
                }
                .xml(),
            )
            .await;
            assert_eq!(
                created.status, 201,
                "creating the cache policy answered {}: {}",
                created.status, created.raw
            );
            let id = created
                .root()
                .find_text("Id")
                .expect("the created cache policy reports an id")
                .to_string();
            format!("<CachePolicyId>{id}</CachePolicyId>")
        }
        Spans::Behavior => format!(
            "<ForwardedValues><QueryString>false</QueryString>\
             <Cookies><Forward>none</Forward></Cookies>\
             <Headers><Quantity>0</Quantity></Headers>\
             <QueryStringCacheKeys><Quantity>0</Quantity></QueryStringCacheKeys>\
             </ForwardedValues>\
             <MinTTL>{MIN_TTL}</MinTTL><DefaultTTL>{DEFAULT_TTL}</DefaultTTL>\
             <MaxTTL>{MAX_TTL}</MaxTTL>"
        ),
    }
}

/// One deployed distribution whose behaviour states its spans as `spans` says, over its own
/// bucket holding `objects` (each a key and the `Cache-Control` the origin states about it):
/// the distribution's id, the `DomainName` it published, and the bucket its origin is.
async fn staged(
    emulator: &Emulator,
    spans: Spans,
    objects: &[(&str, Option<&str>)],
) -> (String, String, String) {
    let case = spans.name();
    let bucket = spans.name();
    emulator.staged_bucket(case, bucket).await;
    for (key, cache_control) in objects {
        emulator
            .staged_object(case, bucket, key, FIRST, *cache_control)
            .await;
    }
    let control = create_origin_access_control(&emulator.router, case, case).await;
    let (id, domain, arn) = create_distribution_over_bucket(
        &emulator.router,
        case,
        case,
        bucket,
        &control,
        &behavior_spans(emulator, spans).await,
    )
    .await;
    emulator.granted(case, bucket, &arn).await;
    awaited_deployment(&emulator.router, case, &id).await;
    (id, domain, bucket.to_string())
}

/// One row: the `Cache-Control` the origin states about its own object, and the span the edge
/// holds that answer for.
struct Row {
    case: &'static str,
    key: &'static str,
    /// What the origin states in `Cache-Control`, or `None` for an object it states nothing about.
    stated: Option<&'static str>,
    /// The span the edge must hold the answer for, in seconds, or `None` for an answer it must
    /// not hold at all.
    held_seconds: Option<u64>,
}

/// The table, under both places a behaviour's spans come from.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no directive at all: the default span, which only this case reaches",
            key: "stated-nothing.txt",
            stated: None,
            held_seconds: Some(DEFAULT_TTL),
        },
        Row {
            case: "a max-age between the bounds: the span the origin stated",
            key: "max-age-inside.txt",
            stated: Some("max-age=120"),
            held_seconds: Some(120),
        },
        Row {
            case: "an s-maxage, which a shared cache reads ahead of the max-age beside it",
            key: "s-maxage.txt",
            stated: Some("s-maxage=200, max-age=5"),
            held_seconds: Some(200),
        },
        Row {
            case: "a max-age under the minimum, which floors it",
            key: "max-age-under.txt",
            stated: Some("max-age=1"),
            held_seconds: Some(MIN_TTL),
        },
        Row {
            case: "a max-age of no time at all, floored the same way",
            key: "max-age-zero.txt",
            stated: Some("max-age=0"),
            held_seconds: Some(MIN_TTL),
        },
        Row {
            case: "a max-age over the maximum, which caps it",
            key: "max-age-over.txt",
            stated: Some("max-age=86400"),
            held_seconds: Some(MAX_TTL),
        },
        Row {
            case: "an argument that is no span, which states no lifetime",
            key: "max-age-nonsense.txt",
            stated: Some("max-age=soon"),
            held_seconds: Some(DEFAULT_TTL),
        },
        Row {
            case: "a directive that forbids storage, held by the minimum span alone",
            key: "no-store.txt",
            stated: Some("no-store"),
            held_seconds: Some(MIN_TTL),
        },
    ]
}

/// The span an answer is held for is the lifetime the origin stated, bounded by the behaviour's
/// minimum and maximum — and the behaviour's default span only where the origin stated none —
/// whether those three spans come from a cache policy or from the behaviour's own members.
#[tokio::test]
async fn the_span_an_answer_is_held_for_is_the_lifetime_the_origin_stated() {
    let emulator = Emulator::new();
    let rows = rows();
    let objects: Vec<(&str, Option<&str>)> =
        rows.iter().map(|row| (row.key, row.stated)).collect();

    for spans in [Spans::CachePolicy, Spans::Behavior] {
        let (id, domain, _bucket) = staged(&emulator, spans, &objects).await;
        for row in &rows {
            let case = format!("{} / {}", spans.name(), row.case);
            let path = format!("/{}", row.key);
            let viewed = view(&emulator.router, "GET", &domain, &path, &[]).await;
            assert_eq!(
                (viewed.status, viewed.body.as_str(), viewed.x_cache()),
                (200, FIRST, MISS),
                "case {case:?}: expected the origin's own bytes on a miss, got {} {:?} {:?}",
                viewed.status,
                viewed.body,
                viewed.x_cache()
            );
            let actual = emulator.held_span(&case, &id, &path);
            let expected = row.held_seconds.map(Duration::from_secs);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the edge to hold the answer for {expected:?}, got \
                 {actual:?}"
            );
        }
    }
}

/// The stated lifetime is what makes the edge ask the ORIGIN again: once it has run out the next
/// viewer request is served the object the origin holds NOW, while an answer held under the
/// behaviour's default span is still served out of the cache.
///
/// The object under a one-second lifetime is polled for rather than waited out, and the object
/// under the default span is read at the end of that poll: both observations are of the
/// conditions, not of the clock.
#[tokio::test]
async fn an_answer_whose_stated_lifetime_has_run_out_is_fetched_from_the_origin_again() {
    let emulator = Emulator::new();
    // A short-lived object and one the origin states nothing about: the first is held for the
    // second it states, the second for the behaviour's default span.
    let short = "short-lived.txt";
    let kept = "stated-nothing.txt";
    let objects: [(&str, Option<&str>); 2] = [(short, Some("max-age=1")), (kept, None)];
    let (_id, domain, bucket) = staged(&emulator, Spans::Behavior, &objects).await;
    let case = "a stated lifetime that runs out";

    for key in [short, kept] {
        let viewed = view(&emulator.router, "GET", &domain, &format!("/{key}"), &[]).await;
        assert_eq!(
            (viewed.status, viewed.body.as_str(), viewed.x_cache()),
            (200, FIRST, MISS),
            "case {case:?} ({key}): expected the origin's first bytes, got {} {:?} {:?}",
            viewed.status,
            viewed.body,
            viewed.x_cache()
        );
    }

    // The origin now holds other bytes for both objects. Nothing told the edge.
    for (key, stated) in objects {
        emulator
            .staged_object(case, &bucket, key, REFRESHED, stated)
            .await;
    }

    // The short-lived answer: polled until the edge serves what the origin holds now.
    let deadline = Instant::now() + READY_DEADLINE;
    let mut last = String::new();
    let mut refreshed = false;
    while Instant::now() < deadline {
        let viewed = view(
            &emulator.router,
            "GET",
            &domain,
            &format!("/{short}"),
            &[],
        )
        .await;
        last = viewed.body.clone();
        if viewed.status == 200 && viewed.body == REFRESHED {
            refreshed = true;
            break;
        }
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
    assert!(
        refreshed,
        "case {case:?}: expected the edge to serve the origin's current bytes {REFRESHED:?} once \
         the stated lifetime had run out, last {last:?}"
    );

    // The answer held under the default span is still the one the edge already has.
    let viewed = view(&emulator.router, "GET", &domain, &format!("/{kept}"), &[]).await;
    assert_eq!(
        (viewed.status, viewed.body.as_str(), viewed.x_cache()),
        (200, FIRST, HIT),
        "case {case:?} ({kept}): expected the copy the edge holds under its default span, got {} \
         {:?} {:?}",
        viewed.status,
        viewed.body,
        viewed.x_cache()
    );
}
