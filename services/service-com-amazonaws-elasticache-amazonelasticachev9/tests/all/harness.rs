// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate drives the emulator through.
//!
//! One `awsQuery` form-urlencoded request is built, dispatched through the same
//! [`MultiServiceRouter`] the production emulator mounts, and only the HTTP
//! status and the XML body are inspected — so a response that is not computed
//! from persisted state cannot pass.
//!
//! It lives here rather than in one suite because both suites send the same
//! kind of request and read the same kind of answer; a second copy of this
//! scanner would be a second thing to keep correct.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use service_com_amazonaws_elasticache_amazonelasticachev9::state;
use service_com_amazonaws_elasticache_amazonelasticachev9::EmulatedService;
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

/// The region every test signs with unless it is exercising the region rule
/// itself.
pub const REGION: &str = "us-east-1";

/// The access key id every suite signs with unless it is exercising WHO a request
/// speaks for: a key neither STS nor IAM holds a record for, which is the position
/// AWS account root credentials are in.
pub const ACCOUNT_KEY_ID: &str = "AKIDEXAMPLE";

/// The API version an `awsQuery` client sends beside `Action`, from the model's
/// service version (`smithy_query(query="service_info")`: `version 2015-02-02`).
pub const API_VERSION: &str = "2015-02-02";

/// The cache node type a suite names when its case is not about node types.
///
/// `CacheNodeType` is a parameter the service requires of a create that
/// provisions nodes of its own, so every suite that creates a cluster or a
/// replication group names one. This is one of the current-generation types the
/// member's own documentation publishes
/// (`smithy_query(query="doc",shape="CreateCacheClusterMessage",member="CacheNodeType")`),
/// held here once so a suite that does not care which type it runs on does not
/// write down a second one.
pub const TEST_NODE_TYPE: &str = "cache.t3.micro";

/// Long enough past [`state::CLUSTER_TRANSITION`] that the clock owes a
/// resource its next status; the handlers never sleep, so only the test waits.
///
/// Derived from the span itself, so a suite that waits for a transition to fall
/// due keeps waiting long enough when that span changes.
pub const PAST_TRANSITION: Duration =
    Duration::from_millis(state::CLUSTER_TRANSITION.as_millis() as u64 + 250);

/// A span far longer than any transition this service schedules, for a suite that
/// reads a record while the clock still owes it a change.
pub const HELD_OUT_OF_REACH: Duration = Duration::from_secs(60 * 60);

/// Move every change the clock owes this store to `due_in` from now.
///
/// Every record that carries a due instant is rewritten, so a suite can hold its
/// resource in the transient phase whatever that phase is
/// ([`HELD_OUT_OF_REACH`]) and then make the whole outstanding set due at once
/// ([`Duration::ZERO`]). Only the DUE INSTANT moves: the status the clock owes,
/// and everything else the record carries, is the handler's own.
///
/// This is how a suite puts a resource in a transient phase for longer than the
/// span the clock owes it. That span ([`state::CLUSTER_TRANSITION`]) out-lasts
/// the request that follows the call which scheduled the transition, so a suite
/// about what the service answers WHILE a resource is being created, changed or
/// removed can read it straight back — but a suite that needs the phase to hold
/// across several calls, or across a wait, holds the change out of reach rather
/// than racing the span, and what it asserts then does not depend on how fast
/// the host runs.
pub fn every_pending_change_due_in(emulator: &Emulator, due_in: Duration) {
    let due = ::std::time::SystemTime::now() + due_in;
    emulator.with_state(|store| {
        for cluster in store.cache_clusters.values_mut() {
            if let Some(pending) = cluster.pending_status.as_mut() {
                pending.due = due;
            }
        }
        for group in store.replication_groups.values_mut() {
            if let Some(pending) = group.pending_status.as_mut() {
                pending.due = due;
            }
            if group.pending_removal.is_some() {
                group.pending_removal = Some(due);
            }
            // The data sync a `StartMigration` opened carries a due instant of
            // its own — when the service has ESTABLISHED it — so a suite about
            // what a cutover is answered before then holds that instant out of
            // reach too.
            if let Some(migration) = group.migration.as_mut() {
                if migration.established_due.is_some() {
                    migration.established_due = Some(due);
                }
            }
        }
        for group in store.global_replication_groups.values_mut() {
            if let Some(pending) = group.pending_status.as_mut() {
                pending.due = due;
            }
        }
        for snapshot in store.snapshots.values_mut() {
            if let Some(pending) = snapshot.pending_status.as_mut() {
                pending.due = due;
            }
            if snapshot.pending_removal.is_some() {
                snapshot.pending_removal = Some(due);
            }
        }
        for cache in store.serverless_caches.values_mut() {
            if let Some(pending) = cache.pending_status.as_mut() {
                pending.due = due;
            }
            if cache.pending_deletion.is_some() {
                cache.pending_deletion = Some(due);
            }
            // An integration configured on the cache carries a due instant of
            // its own — when the service has ENABLED it — so a suite about what
            // a cache answers before then holds that instant too.
            for integration in &mut cache.integrations {
                if let Some(pending) = integration.pending_status.as_mut() {
                    pending.due = due;
                }
            }
        }
        for snapshot in store.serverless_cache_snapshots.values_mut() {
            if let Some(pending) = snapshot.pending_status.as_mut() {
                pending.due = due;
            }
            if snapshot.pending_removal.is_some() {
                snapshot.pending_removal = Some(due);
            }
        }
        for user in store.users.values_mut() {
            if let Some(pending) = user.pending_status.as_mut() {
                pending.due = due;
            }
            if user.pending_removal.is_some() {
                user.pending_removal = Some(due);
            }
        }
        for group in store.user_groups.values_mut() {
            if let Some(pending) = group.pending_status.as_mut() {
                pending.due = due;
            }
            if group.pending_removal.is_some() {
                group.pending_removal = Some(due);
            }
        }
        for reservation in store.reserved_cache_nodes.values_mut() {
            if let Some(pending) = reservation.pending_state.as_mut() {
                pending.due = due;
            }
        }
    });
}

/// Hold every change the clock owes out of reach, leaving each resource in the
/// transient phase the call that started its work put it in; see
/// [`every_pending_change_due_in`].
pub fn hold_pending_changes(emulator: &Emulator) {
    every_pending_change_due_in(emulator, HELD_OUT_OF_REACH);
}

/// Carry every change the clock owes this store through now, exactly as the next
/// call of the service would once those changes fall due.
///
/// The outstanding change falls due first ([`every_pending_change_due_in`]) and is
/// materialised here, so a suite that has just started work on a resource —
/// taking a snapshot, failing a shard over, creating a group — reaches the settled
/// resource without waiting out the span the clock owes it
/// ([`state::CLUSTER_TRANSITION`]). For a suite that is about to put ONE resource
/// in a transient phase and hold it there, this leaves nothing else outstanding,
/// so [`hold_pending_changes`] then holds only what the call that follows made
/// transient.
pub fn settle_pending_changes(emulator: &Emulator) {
    every_pending_change_due_in(emulator, Duration::ZERO);
    let now = ::std::time::SystemTime::now();
    emulator.with_state(|store| {
        store.settle_all(now);
        store.settle_user_groups(now);
        store.settle_replication_groups(now);
        store.settle_global_replication_groups(now);
    });
}

/// Make every change the clock owes due now, so the next call materialises it;
/// the other half of [`hold_pending_changes`].
pub fn release_pending_changes(emulator: &Emulator) {
    every_pending_change_due_in(emulator, Duration::ZERO);
}

/// How long a suite waits for a status change the CLOCK owes before it gives up,
/// and how often it looks again while it waits.
///
/// Generous on purpose: a suite waits for the CONDITION it needs — through
/// [`poll_until`] — rather than for a fixed time, so what it asserts does not
/// depend on how fast or how loaded the host is.
pub const SETTLE_DEADLINE: Duration = Duration::from_secs(30);
/// See [`SETTLE_DEADLINE`].
pub const LOOK_AGAIN_AFTER: Duration = Duration::from_millis(25);

/// Send `action` in `region` until the answer satisfies `holds`, and answer with
/// that reply.
///
/// The one place a suite of this crate waits for a status change the clock owes:
/// it looks again until the condition holds or [`SETTLE_DEADLINE`] passes, and a
/// deadline that passes fails with the last answer quoted. `description` says
/// what was being waited for.
pub async fn poll_until(
    emulator: &Emulator,
    region: &str,
    case: &str,
    description: &str,
    action: &str,
    request: &[(String, String)],
    holds: impl Fn(&Reply) -> bool,
) -> Reply {
    let deadline = ::std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let reply = emulator.call_in(region, action, request).await;
        if holds(&reply) {
            return reply;
        }
        assert!(
            ::std::time::Instant::now() < deadline,
            "case {case:?}: {description} never came about; the last {action} answered {} {}",
            reply.status,
            reply.body
        );
        ::tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}

/// One refusal row: the case name, the form pairs that make the request, and
/// the HTTP status plus `awsQuery` error code it must be answered with.
pub type RefusalCase = (
    &'static str,
    &'static [(&'static str, &'static str)],
    u16,
    &'static str,
);

/// Whether `address` is an address of the EMULATOR's OWN — which is what every
/// endpoint a cache publishes has to be.
///
/// A client dials exactly the address it was handed, and nothing on the machine
/// this emulator runs on resolves a name under ElastiCache's own DNS zone; the
/// core binds its resource listeners on the operator-chosen interface (loopback
/// by default), so an address a cache publishes is an IP literal or a loopback
/// name. The suites that read an endpoint assert this rather than a hostname
/// shape, because the port is assigned by the core and the host is wherever the
/// operator bound it.
pub fn is_own_host(address: &str) -> bool {
    let unbracketed = address.trim_start_matches('[').trim_end_matches(']');
    unbracketed.parse::<::std::net::IpAddr>().is_ok() || address == "localhost"
}

/// Assert that `address` and `port` are an endpoint of this emulator's own: a
/// host it answers on and a port the core assigned.
pub fn assert_own_endpoint(case: &str, what: &str, address: &str, port: Option<&str>) {
    assert!(
        is_own_host(address),
        "case {case}: {what} must be an address of the emulator's own, got {address:?}"
    );
    let parsed = port.and_then(|port| port.parse::<u16>().ok());
    assert!(
        parsed.is_some_and(|port| port > 0),
        "case {case}: {what} must carry the port the cache answers on, got {port:?}"
    );
}

/// Assert `reply` is the refusal `status` / `code` carrying exactly `message`.
///
/// The one copy of the assertion the refusal-wording suites of this crate share: a
/// refusal's whole observable surface is its status, its `awsQuery` code and its
/// sentence, so a suite that checks one of the three and not the others passes a
/// refusal a client cannot act on. `case` names the row.
pub fn assert_refusal(case: &str, reply: &Reply, status: u16, code: &str, message: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (status, code),
        "case {case:?}: expected {status} {code}, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        message,
        "case {case:?}: expected message {message:?}, got {:?}",
        reply.error_message()
    );
}

/// One wire answer, reduced to what a client branches on: the status, and every
/// text node in the XML body keyed by its element path.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub nodes: Vec<(String, String)>,
}

impl Reply {
    /// The first text node whose element path ends in `suffix`.
    pub fn text(&self, suffix: &str) -> Option<&str> {
        self.all(suffix).into_iter().next()
    }

    /// Every text node whose element path ends in `suffix`, in document order.
    pub fn all(&self, suffix: &str) -> Vec<&str> {
        self.nodes
            .iter()
            .filter(|(path, _)| path == suffix || path.ends_with(&format!("/{suffix}")))
            .map(|(_, text)| text.as_str())
            .collect()
    }

    /// The `awsQuery` error code (`<ErrorResponse><Error><Code>`), or the empty
    /// string on a success.
    pub fn error_code(&self) -> &str {
        self.text("Error/Code").unwrap_or_default()
    }

    /// The `awsQuery` error message, or the empty string on a success.
    pub fn error_message(&self) -> &str {
        self.text("Error/Message").unwrap_or_default()
    }

    /// The pagination `Marker` a describe answered with, if any. `Marker` is the
    /// only element of that name in either describe's output.
    pub fn marker(&self) -> Option<&str> {
        self.text("Marker")
    }
}

/// Resolve the five predefined XML entities in an element's text.
///
/// `pub(crate)` for the same reason as the rest of this harness: a suite that reads an
/// element out of the RAW body (because the scanner below drops an element whose text
/// is empty) resolves its entities the one way rather than carrying a second copy.
pub(crate) fn unescape_xml(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        // `&amp;` last, so `&amp;lt;` does not become `<`.
        .replace("&amp;", "&")
}

/// Whether `body` carries an element named `name` at all — including one that is present
/// and EMPTY (`<name></name>`, `<name/>`) or carrying attributes (`<name xmlns="…">`).
///
/// `pub(crate)` for the same reason as the rest of this harness: the scanner below drops an
/// element whose text is empty, so a suite that asserts a list or a structure holding
/// nothing was SERIALIZED has to read the raw body, and reads it the one way rather than
/// carrying a second copy of this.
pub(crate) fn carries_element(body: &str, name: &str) -> bool {
    [
        format!("<{name}>"),
        format!("<{name}/>"),
        format!("<{name} "),
    ]
    .iter()
    .any(|open| body.contains(open.as_str()))
}

/// Parse an XML document into `(element path, text)` pairs in document order.
///
/// Deliberately a local scanner rather than an XML-crate dependency: adding one
/// would put this crate's only edit into `Cargo.lock`, the one file every
/// concurrent fix also edits, and that collision is what kept the merge gate's
/// squash refusing this change. The bodies parsed here are the awsQuery
/// envelopes the vendored protocol serializer emits — elements, text and an
/// `xmlns` attribute — so a scanner over `<` / `>` reads them exactly.
fn xml_text_nodes(body: &str) -> Vec<(String, String)> {
    let mut path: Vec<String> = Vec::new();
    let mut out: Vec<(String, String)> = Vec::new();
    let mut rest = body;
    while let Some(open) = rest.find('<') {
        // Whatever precedes the tag is the text of the element now open.
        let text = rest[..open].trim();
        if !text.is_empty() && !path.is_empty() {
            out.push((path.join("/"), unescape_xml(text)));
        }
        rest = &rest[open + 1..];
        let Some(close) = rest.find('>') else {
            panic!("unterminated XML tag in response body:\n{body}");
        };
        let tag = &rest[..close];
        rest = &rest[close + 1..];
        // `<?xml …?>`, `<!-- … -->` and `<![CDATA[…]]>` open no element.
        if tag.starts_with('?') || tag.starts_with('!') {
            continue;
        }
        if tag.starts_with('/') {
            assert!(
                path.pop().is_some(),
                "closing tag with no open element in response body:\n{body}"
            );
            continue;
        }
        let self_closing = tag.ends_with('/');
        let name = tag
            .trim_end_matches('/')
            .split_whitespace()
            .next()
            .unwrap_or_default();
        if !self_closing {
            path.push(name.to_string());
        }
    }
    out
}

/// Percent-encode one form value, leaving only the unreserved characters.
fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// One subnet the ACCOUNT has, as Amazon EC2 — the service that owns subnets,
/// and the one every ElastiCache placement is resolved against — reports it.
///
/// Nothing here derives an identifier, a zone or a VPC: a suite that needs a
/// subnet id asks the owner for one, exactly as the service under test does.
#[derive(Debug, Clone)]
pub struct AccountSubnet {
    pub id: String,
    pub zone: String,
    pub vpc: String,
    /// Whether this is the default subnet for its Availability Zone — EC2's own
    /// `DefaultForAz`, which is what makes it one of the subnets ElastiCache
    /// selects for a request that names none.
    pub default_for_az: bool,
}

/// An emulator with this service mounted on a fresh engine BESIDE Amazon EC2,
/// driven in-process through the production router.
///
/// EC2 comes with it because a cache subnet group and a serverless cache are
/// placed in the subnets the ACCOUNT has, and the only answer to which subnets
/// those are is the service that owns them — so a deployment without it serves
/// no placement at all.
pub struct Emulator {
    router: MultiServiceRouter,
    /// The SINGLE shared engine the router hands to every operation. Held so a
    /// suite can seed a resource whose own create operation is still a
    /// scaffolded 501, and then assert over the wire that the handlers answer
    /// FROM that stored state.
    engine: Arc<engine::InMemoryEngine>,
    /// The in-process seam an AWS SDK client reaches this emulator through, so a
    /// suite can ask EC2 what the account's network is per Region.
    callback: EmbeddedEmulator,
}

impl Emulator {
    pub fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these tests are about; the router still routes
        // by the SigV4 credential scope below, which is also where an ARN's
        // region comes from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
            Arc::clone(&engine),
        ));
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self {
            router,
            engine,
            callback,
        }
    }

    /// The same emulator with IAM and STS mounted BESIDE this service on the one
    /// shared engine, the way the assembled emulator mounts them.
    ///
    /// That is the only way a suite about AUTHORIZATION reaches the records the
    /// decision is taken from: the role and its policies are IAM's own records,
    /// and the session an access key id speaks for is STS's, which is where the
    /// gate ahead of every operation resolves both.
    pub fn with_identity() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self {
            router,
            engine,
            callback,
        }
    }

    /// The same emulator with KMS mounted BESIDE this service on the one shared
    /// engine, the way the assembled emulator mounts them.
    ///
    /// That is the only way a suite about an encrypted resource reaches the real
    /// key records `kms CreateKey` / `CreateAlias` wrote, because resolving the
    /// `KmsKeyId` a request names is a call into KMS.
    pub fn with_kms() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(&engine)),
        );
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self {
            router,
            engine,
            callback,
        }
    }

    /// The same emulator with Amazon S3 mounted BESIDE this service on the one
    /// shared engine, the way the assembled emulator mounts them.
    ///
    /// That is the only way a suite about an EXPORT reaches the objects the
    /// export writes, because a bucket and the object in it belong to Amazon S3:
    /// what this service reports as exported is read back through that
    /// service's own operations.
    pub fn with_s3() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self {
            router,
            engine,
            callback,
        }
    }

    /// The same emulator with Amazon SNS and Amazon SQS mounted BESIDE this
    /// service on the one shared engine, the way the assembled emulator mounts
    /// them.
    ///
    /// Both because they are the DELIVERY PATH a notification travels: a cluster
    /// event is published to the topic the cluster's notification configuration
    /// names, and Amazon SNS fans that publish out to the queue subscribed to the
    /// topic. A suite about an event notification therefore reads what it
    /// published back from the services that own topics, subscriptions and
    /// queues, rather than from this service's own state.
    pub fn with_sns() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
            Arc::clone(&engine),
        ));
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self {
            router,
            engine,
            callback,
        }
    }

    /// An Amazon SNS client for `region` over the in-process callback this
    /// emulator routes on — the same seam the service itself publishes a
    /// notification through, so a suite's setup reaches SNS exactly where the
    /// handler does.
    pub fn sns(&self, region: &str) -> ::aws_sdk_sns::Client {
        let config = ::aws_sdk_sns::Config::builder()
            .behavior_version(::aws_sdk_sns::config::BehaviorVersion::latest())
            .region(::aws_sdk_sns::config::Region::new(region.to_string()))
            .endpoint_url(format!("https://sns.{region}.amazonaws.com"))
            .credentials_provider(self.callback.aws_credentials_provider())
            .http_client(self.callback.http_client())
            .retry_config(::aws_sdk_sns::config::retry::RetryConfig::disabled())
            .timeout_config(::aws_sdk_sns::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::aws_sdk_sns::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                ::aws_sdk_sns::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        ::aws_sdk_sns::Client::from_conf(config)
    }

    /// An Amazon SQS client for `region` over the same in-process callback — the
    /// subscriber's end of the delivery path above.
    pub fn sqs(&self, region: &str) -> ::aws_sdk_sqs::Client {
        let config = ::aws_sdk_sqs::Config::builder()
            .behavior_version(::aws_sdk_sqs::config::BehaviorVersion::latest())
            .region(::aws_sdk_sqs::config::Region::new(region.to_string()))
            .endpoint_url(format!("https://sqs.{region}.amazonaws.com"))
            .credentials_provider(self.callback.aws_credentials_provider())
            .http_client(self.callback.http_client())
            .retry_config(::aws_sdk_sqs::config::retry::RetryConfig::disabled())
            .timeout_config(::aws_sdk_sqs::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::aws_sdk_sqs::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                ::aws_sdk_sqs::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        ::aws_sdk_sqs::Client::from_conf(config)
    }

    /// The ONE engine every service mounted here shares — for a suite that reads
    /// back what ANOTHER service holds (the deliveries Amazon SNS made) through
    /// that service's own store.
    pub fn engine(&self) -> &Arc<engine::InMemoryEngine> {
        &self.engine
    }

    /// An Amazon S3 client for `region` over the in-process callback this
    /// emulator routes on: no socket is opened, and the endpoint URL is only the
    /// address that transport routes by.
    ///
    /// The emulator serves every service on one endpoint, so the bucket travels
    /// in the PATH rather than in a per-bucket host — the same addressing the
    /// service under test writes its export through.
    pub fn s3(&self, region: &str) -> ::aws_sdk_s3::Client {
        let config = ::aws_sdk_s3::Config::builder()
            .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
            .region(::aws_sdk_s3::config::Region::new(region.to_string()))
            .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
            .force_path_style(true)
            .credentials_provider(self.callback.aws_credentials_provider())
            .http_client(self.callback.http_client())
            .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
            .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                ::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        ::aws_sdk_s3::Client::from_conf(config)
    }

    /// An EC2 client for `region` over the in-process callback this emulator
    /// routes on: no socket is opened, and the endpoint URL is only the address
    /// that transport routes by.
    ///
    /// The very seam this service's own subnet resolution reaches EC2 through,
    /// so a suite that builds a network with `ec2:CreateVpc`/`ec2:CreateSubnet`
    /// mints it exactly where a placement will look for it.
    pub fn ec2(&self, region: &str) -> ::aws_sdk_ec2::Client {
        let config = ::aws_sdk_ec2::Config::builder()
            .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
            .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
            .endpoint_url(format!("https://ec2.{region}.amazonaws.com"))
            .credentials_provider(self.callback.aws_credentials_provider())
            .http_client(self.callback.http_client())
            .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
            .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                ::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        ::aws_sdk_ec2::Client::from_conf(config)
    }

    /// The subnets the account has in `region`, as EC2 reports them, ordered by
    /// Availability Zone.
    ///
    /// ASKED, never assumed: these are the ids the owner of subnets answers
    /// with for this account, which is also what the service under test resolves
    /// a placement against.
    pub async fn account_subnets_in(&self, region: &str) -> Vec<AccountSubnet> {
        let described = self
            .ec2(region)
            .describe_subnets()
            .send()
            .await
            .expect("EC2 must report the account's subnets");
        let mut reported: Vec<AccountSubnet> = described
            .subnets()
            .iter()
            .filter_map(|subnet| {
                Some(AccountSubnet {
                    id: subnet.subnet_id()?.to_string(),
                    zone: subnet.availability_zone()?.to_string(),
                    vpc: subnet.vpc_id()?.to_string(),
                    default_for_az: subnet.default_for_az().unwrap_or(false),
                })
            })
            .filter(|subnet| subnet.zone.starts_with(region))
            .collect();
        reported.sort_by(|left, right| left.zone.cmp(&right.zone));
        assert!(
            reported.len() >= 2,
            "the account must have at least two subnets in {region} for these cases, got \
             {reported:?}"
        );
        reported
    }

    /// The subnets the account has in [`REGION`], which is the Region every
    /// suite signs for unless it is exercising the Region rule itself.
    pub async fn account_subnets(&self) -> Vec<AccountSubnet> {
        self.account_subnets_in(REGION).await
    }

    /// Mint `count` VPC security groups in `region` through `ec2:CreateSecurityGroup` and
    /// answer the ids EC2 reported for them, in the order they were created.
    ///
    /// A security group is an Amazon EC2 resource and its identifier is EC2's to mint, so a
    /// suite that needs one ASKS the owner for it — exactly where the service under test
    /// will look for it. `label` keeps the group names of one suite apart from another's,
    /// because "you can't have two security groups for the same VPC with the same name".
    pub async fn security_groups_in(&self, region: &str, label: &str, count: usize) -> Vec<String> {
        let mut minted = Vec::with_capacity(count);
        for index in 0..count {
            minted.push(
                self.security_group_named_in(region, &format!("{label}-{index}"))
                    .await,
            );
        }
        minted
    }

    /// [`Self::security_groups_in`] for [`REGION`].
    pub async fn security_groups(&self, label: &str, count: usize) -> Vec<String> {
        self.security_groups_in(REGION, label, count).await
    }

    /// The id of the account's security group called `name` in `region`, created through
    /// `ec2:CreateSecurityGroup` when the account has no group of that name yet.
    ///
    /// A static table can name the GROUP it means but not its id, because the id is EC2's
    /// to mint: this is what turns the one into the other, and asking twice for one name
    /// answers the same real id both times.
    pub async fn security_group_named_in(&self, region: &str, name: &str) -> String {
        let ec2 = self.ec2(region);
        let filter = ::aws_sdk_ec2::types::Filter::builder()
            .name("group-name")
            .values(name)
            .build();
        let described = ec2
            .describe_security_groups()
            .filters(filter)
            .send()
            .await
            .expect("EC2 must report the account's security groups");
        if let Some(id) = described
            .security_groups()
            .iter()
            .find_map(::aws_sdk_ec2::types::SecurityGroup::group_id)
        {
            return id.to_string();
        }
        ec2.create_security_group()
            .group_name(name)
            .description(format!("the {name} group of this suite"))
            .send()
            .await
            .expect("EC2 must create the security group")
            .group_id()
            .expect("CreateSecurityGroup must report the group id it minted")
            .to_string()
    }

    /// [`Self::security_group_named_in`] for [`REGION`].
    pub async fn security_group_named(&self, name: &str) -> String {
        self.security_group_named_in(REGION, name).await
    }

    /// The id of the DEFAULT security group of `vpc` in `region` — the one EC2 creates with
    /// every VPC ("your VPC automatically comes with a default security group").
    ///
    /// A suite asserting which groups guard a resource whose request named none asks the
    /// OWNER which group that is, exactly where the service under test looks for it, rather
    /// than writing an id down.
    pub async fn default_security_group_of_in(&self, region: &str, vpc: &str) -> String {
        let in_vpc = ::aws_sdk_ec2::types::Filter::builder()
            .name("vpc-id")
            .values(vpc)
            .build();
        let named_default = ::aws_sdk_ec2::types::Filter::builder()
            .name("group-name")
            .values("default")
            .build();
        self.ec2(region)
            .describe_security_groups()
            .filters(in_vpc)
            .filters(named_default)
            .send()
            .await
            .expect("EC2 must report the account's security groups")
            .security_groups()
            .iter()
            .find_map(::aws_sdk_ec2::types::SecurityGroup::group_id)
            .unwrap_or_else(|| panic!("the VPC {vpc} must come with a default security group"))
            .to_string()
    }

    /// [`Self::default_security_group_of_in`] for [`REGION`].
    pub async fn default_security_group_of(&self, vpc: &str) -> String {
        self.default_security_group_of_in(REGION, vpc).await
    }

    /// Delete the security group `group_id` through `ec2:DeleteSecurityGroup`, so the id
    /// becomes one the account HELD and holds no longer.
    pub async fn delete_security_group(&self, group_id: &str) {
        self.ec2(REGION)
            .delete_security_group()
            .group_id(group_id)
            .send()
            .await
            .expect("EC2 must delete the security group");
    }

    /// One inbound rule, as `ec2:AuthorizeSecurityGroupIngress` /
    /// `ec2:RevokeSecurityGroupIngress` carry it: a protocol, a port range where the rule
    /// states one, and a source — an address range, another security group, or both.
    ///
    /// The ONE place a suite of this crate builds such a permission, so the rule a case
    /// authorizes and the rule it revokes are the same rule.
    pub fn ingress_permission(
        protocol: &str,
        ports: Option<(i32, i32)>,
        cidr: Option<&str>,
        source_group: Option<&str>,
    ) -> ::aws_sdk_ec2::types::IpPermission {
        let mut permission = ::aws_sdk_ec2::types::IpPermission::builder().ip_protocol(protocol);
        if let Some((from, to)) = ports {
            permission = permission.from_port(from).to_port(to);
        }
        if let Some(cidr) = cidr {
            permission = permission.ip_ranges(
                ::aws_sdk_ec2::types::IpRange::builder()
                    .cidr_ip(cidr)
                    .build(),
            );
        }
        if let Some(group_id) = source_group {
            permission = permission.user_id_group_pairs(
                ::aws_sdk_ec2::types::UserIdGroupPair::builder()
                    .group_id(group_id)
                    .build(),
            );
        }
        permission.build()
    }

    /// Write `permission` as an inbound rule of the security group `group_id`, through
    /// `ec2:AuthorizeSecurityGroupIngress` — the call a client makes so the clients of a
    /// resource that group guards can reach it.
    ///
    /// The rule lands where the service under test reads it: the owner of security groups, in
    /// the account's own inventory.
    pub async fn authorize_ingress(
        &self,
        group_id: &str,
        permission: ::aws_sdk_ec2::types::IpPermission,
    ) {
        self.ec2(REGION)
            .authorize_security_group_ingress()
            .group_id(group_id)
            .ip_permissions(permission)
            .send()
            .await
            .expect("EC2 must authorize the ingress rule");
    }

    /// Take `permission` back off the security group `group_id`, through
    /// `ec2:RevokeSecurityGroupIngress`, and answer what EC2 reported for the revoke
    /// (`Return`).
    pub async fn revoke_ingress(
        &self,
        group_id: &str,
        permission: ::aws_sdk_ec2::types::IpPermission,
    ) -> bool {
        self.ec2(REGION)
            .revoke_security_group_ingress()
            .group_id(group_id)
            .ip_permissions(permission)
            .send()
            .await
            .expect("EC2 must answer the revoke")
            .r#return()
            .unwrap_or(false)
    }

    /// Authorize TCP on `port` from every address, on the security group `group_id`: the rule
    /// a client writes so the clients of the cache that group guards can open a connection to
    /// it at all.
    pub async fn authorize_tcp_ingress_from_anywhere(&self, group_id: &str, port: i32) {
        self.authorize_ingress(
            group_id,
            Self::ingress_permission("tcp", Some((port, port)), Some("0.0.0.0/0"), None),
        )
        .await;
    }

    /// Send one KMS call through the same router and answer its HTTP status and
    /// parsed body.
    ///
    /// KMS is an awsJson1_1 service addressed by the `TrentService` target
    /// prefix; the router routes on the credential scope's service field, which
    /// is why these headers differ from the `awsQuery` ones above.
    pub async fn kms(
        &self,
        operation: &str,
        body: &serde_json::Value,
    ) -> (u16, serde_json::Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/kms/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=deadbeef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("kms.{REGION}.amazonaws.com"))
            .header("authorization", authorization)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("TrentService.{operation}"))
            .body(Bytes::from(body.to_string()))
            .expect("build the KMS request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
        };
        (status, parsed)
    }

    /// Run `edit` against this service's own store in the shared engine — the
    /// same store the handlers read and write.
    pub fn with_state<R>(
        &self,
        edit: impl FnOnce(&mut state::ComAmazonawsElasticacheAmazonelasticachev9State) -> R,
    ) -> R {
        let store = self.store();
        let mut guard = store.lock();
        edit(&mut guard)
    }

    /// The store this emulator's service reads and writes — for a suite that has
    /// to put a record in the store that no implemented operation can create
    /// yet.
    pub fn store(
        &self,
    ) -> engine::ServiceState<state::ComAmazonawsElasticacheAmazonelasticachev9State> {
        self.engine
            .state::<state::ComAmazonawsElasticacheAmazonelasticachev9State>()
    }

    /// Send one `awsQuery` call: `Action`, `Version` and the given form pairs,
    /// signed with [`ACCOUNT_KEY_ID`].
    pub async fn call_in(
        &self,
        region: &str,
        action: &str,
        params: &[(String, String)],
    ) -> Reply {
        self.call_signed_by(ACCOUNT_KEY_ID, region, action, params)
            .await
    }

    /// The same call signed with `access_key_id` — what names the principal the
    /// request speaks for, and so what the authorization gate ahead of every
    /// operation decides the call on.
    pub async fn call_signed_by(
        &self,
        access_key_id: &str,
        region: &str,
        action: &str,
        params: &[(String, String)],
    ) -> Reply {
        let mut body = format!("Action={action}&Version={API_VERSION}");
        for (key, value) in params {
            body.push('&');
            body.push_str(key);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{region}/elasticache/\
             aws4_request, SignedHeaders=host, Signature=deadbeef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("elasticache.{region}.amazonaws.com"))
            .header("authorization", authorization)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Bytes::from(body))
            .expect("build request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = String::from_utf8_lossy(&bytes).into_owned();
        let nodes = if body.is_empty() {
            Vec::new()
        } else {
            xml_text_nodes(&body)
        };
        Reply {
            status,
            body,
            nodes,
        }
    }

    pub async fn call(&self, action: &str, params: &[(String, String)]) -> Reply {
        self.call_in(REGION, action, params).await
    }
}

/// `[("A", "b")]` as the owned pairs [`Emulator::call`] takes.
pub fn params(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The form pairs of a `CreateCacheCluster` for the cluster `id`, with `extra`
/// beside them.
///
/// `CacheNodeType` is a parameter the service requires of a create that
/// provisions nodes of its own, so a caller whose case is not about the node
/// type gets [`TEST_NODE_TYPE`] and one that names its own keeps it. One place,
/// so every suite that creates a cluster builds the same request.
pub fn cluster_create_params(id: &str, extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut pairs = params(&[("CacheClusterId", id)]);
    if !extra.iter().any(|(member, _)| *member == "CacheNodeType") {
        pairs.extend(params(&[("CacheNodeType", TEST_NODE_TYPE)]));
    }
    pairs.extend(params(extra));
    pairs
}

/// `pairs` as a request, with `CacheNodeType` added when they name none — for a
/// row that writes the whole `CreateCacheCluster` request out, identifier
/// included.
///
/// Same reason as [`cluster_create_params`]: the node type is a parameter the
/// service requires of such a create, so a row about something else still has
/// to carry one.
pub fn with_node_type(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut request = params(pairs);
    if !pairs.iter().any(|(member, _)| *member == "CacheNodeType") {
        request.extend(params(&[("CacheNodeType", TEST_NODE_TYPE)]));
    }
    request
}

/// Take one snapshot named `name` of `cluster` and assert it was accepted.
///
/// Also the one place the wait these tests use is checked against the span the
/// service publishes: a wait that did not out-last
/// [`state::SNAPSHOT_TRANSITION`] would let a test read a snapshot the clock had
/// not settled yet and report it as a failure of the handler.
pub async fn snapshot(emulator: &Emulator, name: &str, cluster: &str) {
    assert!(
        PAST_TRANSITION > state::SNAPSHOT_TRANSITION,
        "these tests wait {PAST_TRANSITION:?}, which must out-last the snapshot span ({:?}) this \
         service publishes",
        state::SNAPSHOT_TRANSITION
    );
    let created = emulator
        .call(
            "CreateSnapshot",
            &params(&[("SnapshotName", name), ("CacheClusterId", cluster)]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {name}: CreateSnapshot must answer 200, got {} {}",
        created.status, created.body
    );
}

/// Read one snapshot back by name, asserting the describe itself succeeded.
pub async fn describe_snapshot(emulator: &Emulator, name: &str) -> Reply {
    let described = emulator
        .call("DescribeSnapshots", &params(&[("SnapshotName", name)]))
        .await;
    assert_eq!(
        described.status, 200,
        "case {name}: DescribeSnapshots must answer 200, got {} {}",
        described.status, described.body
    );
    described
}
