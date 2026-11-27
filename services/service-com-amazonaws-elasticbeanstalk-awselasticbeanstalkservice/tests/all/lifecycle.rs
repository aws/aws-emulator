// SPDX-License-Identifier: CC0-1.0

//! The Elastic Beanstalk application / application-version / environment lifecycle,
//! driven end to end over the service's awsQuery HTTP surface.
//!
//! Every test here mounts this one service in the same `MultiServiceRouter` the
//! production binary uses and speaks to it the way an AWS client does: a
//! form-urlencoded `Action=…` body in, an XML envelope out. Nothing in THIS
//! suite reaches into the store directly, so a create that does not persist, or
//! a describe that answers from anything other than the store, fails these
//! tests.
//!
//! The harness ([`Emulator`], [`Answer`]) is `pub(crate)` so the other suites in
//! this test target drive the emulator the same way instead of copying it;
//! [`Emulator::store`] is there for a suite that has to seed a precondition no
//! implemented operation can produce yet.
//!
//! [`Emulator`] and [`Answer`] are `pub(crate)`: every other suite in this one
//! test target (see `tests/all.rs`) drives the service through them rather than
//! standing up a second harness of its own.
//!
//! The two helpers at the bottom — the form-value encoder and the element reader
//! — are written here rather than taken from a crate ON PURPOSE: a dependency
//! would have to be recorded in `Cargo.lock`, the one file this branch shares
//! with every other fix landing on mainline, and that shared hunk is what kept
//! this change's squash merge colliding. Both are a few lines over the
//! emulator's OWN output: the encoder is SigV4's unreserved-set rule, and the
//! reader slices the text between the opening and closing tag of an element in
//! a document this service generated (no CDATA, no comments), allowing the
//! `xmlns` attribute the first member of a list repeats and counting nested
//! tags of the same name so a `<member>` inside a `<member>` is read whole.
//!
//! This module also OWNS that harness for the suites beside it: `Emulator`,
//! `Answer` and the element reader are `pub(crate)`, so the other suites in this
//! test target — `all/operations_role.rs`, `all/solution_stacks.rs`,
//! `all/events.rs` and the rest — drive the same router over the same wire
//! rather than each repeating one.

use std::sync::Arc;
use std::time::Duration;

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;
use smithy_server_runtime::MultiServiceRouter;

/// The service's API version, as the model declares it (`version: 2010-12-01`).
const API_VERSION: &str = "2010-12-01";

/// The region the tests sign with, so every region-derived value (ARNs, CNAMEs)
/// is asserted against something that is not the default.
pub(crate) const TEST_REGION: &str = "us-west-2";

/// The DNS zone the emulator a test drives announces a name of its OWN in: a
/// test's emulator is served at no endpoint of its own, so such a name sits in the
/// zone RFC 6761 reserves for the loopback interface the core binds its listeners
/// on — which is what a resolver answers it with.
pub(crate) const ANNOUNCED_ZONE: &str = "localhost";

/// The zone a CNAME prefix forms its fully qualified DNS NAME in, which
/// `CheckDNSAvailability` REPORTS ON — Elastic Beanstalk's own.
pub(crate) const CNAME_ZONE: &str = "elasticbeanstalk.com";

/// The domain every ADDRESS this service hands a client sits under: this service's
/// own label in the zone above, so an address it publishes leads back to this
/// process rather than to anything of Amazon Web Services' own.
pub(crate) const PUBLISHED_DOMAIN: &str = "elasticbeanstalk.localhost";

/// The access key id every suite signs with unless it is asserting WHO the caller
/// is: a key neither the IAM store nor any session holds a record for, which is
/// the position the account's own published credential is in.
pub(crate) const ACCOUNT_KEY_ID: &str = "AKIDTESTEXAMPLE";

/// A platform for an environment to run on: a `SolutionStackName` from the service's
/// OWN published catalogue (`eb::state::SOLUTION_STACKS`, what
/// `ListAvailableSolutionStacks` reports), derived through the same
/// `SolutionStack::name()` the service reports it by — AWS's `<os> v<version> running
/// <runtime>` form — so it cannot drift from the catalogue or from the member it is
/// sent as.
///
/// `CreateEnvironment` refuses a request that names no configuration template, no
/// solution stack and no platform version ("You must specify either Configuration
/// Template name or Solution Stack name."), so a fixture environment names one of the
/// three; this is the one suites reach for when WHICH platform is not what they are
/// asserting.
pub(crate) static SOLUTION_STACK: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    eb::state::SOLUTION_STACKS
        .first()
        .expect("the service publishes at least one solution stack")
        .name()
});

/// Long enough that a status change the clock owed is due. Derived from the
/// service's own span so the two cannot drift apart.
pub(crate) const PAST_DUE: Duration =
    eb::state::STATUS_TRANSITION_SPAN.saturating_add(Duration::from_millis(250));

/// The emulator under test: this one service, mounted in the router the
/// production binary mounts it in, driven over HTTP.
///
/// `pub(crate)` on this harness and on [`Answer`] / [`elements`] below is what
/// lets a sibling suite in `tests/all/` drive the same emulator the same way
/// instead of writing a second copy of the encoder and the XML reader.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The one shared engine the mounted service's handlers reach their store
    /// through; see [`Emulator::store`].
    engine: Arc<engine::InMemoryEngine>,
}

/// WHICH other services are mounted beside Elastic Beanstalk on the one shared
/// engine, for the constructor that builds an emulator with them answering.
///
/// A struct rather than a row of booleans, so each constructor below says which
/// service it mounts at the point it asks for it.
struct Mounts {
    object_storage: bool,
    notifications: bool,
    identity: bool,
    log_service: bool,
}

impl Mounts {
    /// Elastic Beanstalk alone, which every constructor starts from.
    const NONE: Self = Self {
        object_storage: false,
        notifications: false,
        identity: false,
        log_service: false,
    };
}

impl Emulator {
    pub(crate) fn new() -> Self {
        Self::over_engine(false)
    }

    /// A fresh emulator with the S3 service mounted beside Elastic Beanstalk on the
    /// SAME engine — which is what the assembled emulator does.
    ///
    /// `CreateApplicationVersion` and `CreatePlatformVersion` DOWNLOAD the Amazon S3
    /// archive their request names, by calling object storage in process, so a suite
    /// that drives either one against a real archive needs object storage answering.
    pub(crate) fn with_object_storage() -> Self {
        Self::over_engine(true)
    }

    /// A fresh emulator with object storage and the CloudWatch Logs service
    /// mounted beside Elastic Beanstalk on the SAME engine — which is what the
    /// assembled emulator does.
    ///
    /// An environment whose configuration turns instance log streaming on
    /// PUBLISHES a CloudWatch Logs group per on-instance log file its platform
    /// streams and delivers the application's own output into one of them, by
    /// calling CloudWatch Logs in process, so a suite that asserts the groups,
    /// their retention or their events needs that service answering — and object
    /// storage beside it, because the application whose output is delivered comes
    /// out of the version's source bundle.
    pub(crate) fn with_object_storage_and_logs() -> Self {
        Self::mounting(Mounts {
            object_storage: true,
            log_service: true,
            ..Mounts::NONE
        })
    }

    /// A fresh emulator with the SNS and SQS services mounted beside Elastic
    /// Beanstalk on the SAME engine — which is what the assembled emulator does.
    ///
    /// An environment whose configuration names an Elastic Beanstalk
    /// notification topic has its events PUBLISHED to that topic, by calling
    /// Amazon SNS in process, so a suite that asserts the delivery needs SNS
    /// answering and a queue of its own to read the notification from.
    pub(crate) fn with_notification_services() -> Self {
        Self::mounting(Mounts {
            notifications: true,
            ..Mounts::NONE
        })
    }

    /// A fresh emulator with object storage and the IAM service mounted beside
    /// Elastic Beanstalk on the SAME engine — which is what the assembled
    /// emulator does.
    ///
    /// A request is authorized against the policies the IAM store holds for the
    /// principal its access key id speaks for (`crate::service::authorization`),
    /// so a suite that drives a caller OTHER than the account's own published
    /// credential needs IAM answering — and object storage beside it, because the
    /// creates such a caller makes read a caller-named archive.
    pub(crate) fn with_identity_services() -> Self {
        Self::mounting(Mounts {
            object_storage: true,
            identity: true,
            ..Mounts::NONE
        })
    }

    /// The one shared engine the mounted services reach their stores through, for
    /// a suite that has to SEED a record in ANOTHER mounted service's store (the
    /// IAM role and policy a caller is judged on) before driving this service
    /// over HTTP.
    pub(crate) fn engine(&self) -> &Arc<engine::InMemoryEngine> {
        &self.engine
    }

    /// The one construction path both constructors use, so they cannot drift.
    fn over_engine(with_object_storage: bool) -> Self {
        Self::mounting(Mounts {
            object_storage: with_object_storage,
            ..Mounts::NONE
        })
    }

    /// Mount Elastic Beanstalk, and the other services a suite needs answering
    /// beside it, on one shared engine.
    fn mounting(mounts: Mounts) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(eb::EmulatedService::new(Arc::clone(&engine)));
        if mounts.object_storage {
            router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
                Arc::clone(&engine),
            ));
        }
        if mounts.notifications {
            router.register_service(
                service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
                    Arc::clone(&engine),
                ),
            );
            router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
                Arc::clone(&engine),
            ));
        }
        if mounts.identity {
            router.register_service(
                service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                    Arc::clone(&engine),
                ),
            );
        }
        if mounts.log_service {
            router.register_service(
                service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(
                    Arc::clone(&engine),
                ),
            );
        }
        Self { router, engine }
    }

    /// The `host:port` of the socket the environment plane is served on, which
    /// the first call that needed an address opened.
    ///
    /// NOT a value any operation publishes: the published addresses are NAMES,
    /// carrying neither a port nor a path ([`Emulator::published_cname`]). This is
    /// what resolves them — the socket a client that resolved one of those names
    /// reaches, and the authority a suite that dials the plane by PATH addresses.
    /// Empty before any call has needed an address.
    pub(crate) fn plane_authority(&self) -> String {
        self.store()
            .lock()
            .environment_plane
            .as_ref()
            .map(|endpoint| endpoint.authority().to_string())
            .unwrap_or_default()
    }

    /// The `host:port` and PATH a client of this emulator reaches the environment
    /// carrying the CNAME prefix `prefix` in `region` at: the socket above, then
    /// the plane path naming the Region and that prefix — the port and the path the
    /// published name does not carry, resolved at the listener.
    pub(crate) fn environment_address(&self, region: &str, prefix: &str) -> String {
        format!(
            "{}{}",
            self.plane_authority(),
            eb::state::environment_plane_path(region, prefix)
        )
    }

    /// A name under the zone this emulator announces in that reaches the
    /// environment carrying the CNAME prefix `prefix` in `region`: the plane serves
    /// a request naming a prefix for whichever environment carries that prefix
    /// now, so a suite dials this without having to know the sub-domain the
    /// environment's own `CNAME` carries ([`assert_published_cname`] is where that
    /// value's form is asserted).
    pub(crate) fn dialable_name(&self, region: &str, prefix: &str) -> String {
        format!("{prefix}.{region}.{PUBLISHED_DOMAIN}")
    }

    /// The address an environment with NO load balancer in front of it publishes
    /// as its `EndpointURL`: "the IP address of the instance", which is the host
    /// the environment plane's socket is bound on — that socket's authority without
    /// the port, since the member carries a host and nothing else.
    pub(crate) fn published_instance_address(&self) -> String {
        let authority = self.plane_authority();
        match authority.rsplit_once(':') {
            Some((host, _port)) => host.to_string(),
            None => authority,
        }
    }

    /// The host an environment whose load balancer is named `load_balancer`
    /// publishes as its `EndpointURL`: "the URL to the LoadBalancer" — that load
    /// balancer's own name as one DNS label, the Region, the load-balancing
    /// service's label and this emulator's zone.
    pub(crate) fn published_load_balancer_host(
        &self,
        region: &str,
        load_balancer: &str,
    ) -> String {
        format!(
            "{}.{region}.elb.{ANNOUNCED_ZONE}",
            load_balancer.to_ascii_lowercase()
        )
    }

    /// Dispatch a request the caller built itself, for a suite that drives
    /// ANOTHER mounted service over ITS own wire protocol (the SNS awsQuery
    /// surface, the SQS awsJson one) rather than this service's.
    pub(crate) async fn dispatch(&self, request: http::Request<bytes::Bytes>) -> Answer {
        Answer::of(self.router.dispatch(request).await)
    }

    /// This service's store inside the engine the mounted handlers use — the
    /// same guard `crate::state::service_state` hands a handler.
    ///
    /// For a suite that has to SEED a precondition no implemented operation can
    /// produce yet. The behaviour under test is still driven over HTTP; only the
    /// starting state is written here.
    pub(crate) fn store(
        &self,
    ) -> engine::ServiceState<eb::ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState> {
        eb::state::service_state(&*self.engine).expect("the shared engine carries this service's store")
    }

    /// Call `action` with `params`, signed for [`TEST_REGION`].
    pub(crate) async fn call(&self, action: &str, params: &[(&str, &str)]) -> Answer {
        self.call_in_region(TEST_REGION, action, params).await
    }

    pub(crate) async fn call_in_region(
        &self,
        region: &str,
        action: &str,
        params: &[(&str, &str)],
    ) -> Answer {
        self.call_as(ACCOUNT_KEY_ID, region, action, params).await
    }

    /// Call `action` signed with `access_key_id` in the SigV4 credential scope —
    /// which is what names the principal the request speaks for, and so what the
    /// authorization decision is taken on.
    pub(crate) async fn call_as(
        &self,
        access_key_id: &str,
        region: &str,
        action: &str,
        params: &[(&str, &str)],
    ) -> Answer {
        let mut form = vec![
            format!("Action={}", form_encoded(action)),
            format!("Version={}", form_encoded(API_VERSION)),
        ];
        for (name, value) in params {
            form.push(format!("{}={}", form_encoded(name), form_encoded(value)));
        }
        // The router resolves both the service and the region from the SigV4
        // credential scope, exactly as it does for a real signed request.
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={access_key_id}/20261021/{region}/elasticbeanstalk/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("https://elasticbeanstalk.amazonaws.com/")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", authorization)
            .body(bytes::Bytes::from(form.join("&")))
            .expect("a well-formed request");
        Answer::of(self.router.dispatch(request).await)
    }

    /// Send one path-style request to the emulator's S3 front door, returning the
    /// HTTP status.
    ///
    /// S3 is a REST service: the bucket and key live in the URI and the body is the
    /// object's own bytes, so this is a raw request rather than an `Action=` form.
    /// The credential scope names `s3`, which is what the router claims on.
    pub(crate) async fn s3(&self, method: &str, path: &str, body: &[u8]) -> u16 {
        self.s3_in_region(TEST_REGION, method, path, body)
            .await
            .status
    }

    /// As [`Emulator::s3`], signed for `region` and answering the whole response, for
    /// a suite that reads what object storage handed back (an object's bytes, the
    /// account's buckets) rather than only whether the request succeeded.
    pub(crate) async fn s3_in_region(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> Answer {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTEXAMPLE/20261021/{region}/s3/aws4_request, \
             SignedHeaders=host, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", authorization)
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("a well-formed request");
        Answer::of(self.router.dispatch(request).await)
    }

    /// `GET` `url` with NO `Authorization` header — a request whose only
    /// credential is the one its own query string carries.
    ///
    /// For a suite that follows a PRESIGNED URL this emulator handed out: a
    /// presigned request is signed in the query string, so adding a header
    /// credential would make it a different (and unsigned) request. The URL is
    /// dispatched exactly as the service reported it, so the emulator's own
    /// signature verification is what decides the answer.
    pub(crate) async fn get_url(&self, url: &str) -> Answer {
        let request = http::Request::builder()
            .method("GET")
            .uri(url)
            .body(bytes::Bytes::new())
            .expect("a well-formed request");
        Answer::of(self.router.dispatch(request).await)
    }

    /// Store `body` at `key` in `bucket` in the emulator's object storage, creating
    /// the bucket first, so a create that DOWNLOADS that archive finds it there.
    ///
    /// Both writes go over the S3 front door, so the object an Elastic Beanstalk
    /// create reads is one object storage really holds. Available only on an
    /// emulator built by [`Emulator::with_object_storage`]; on any other the S3
    /// request reaches no service and this panics rather than letting a suite assert
    /// against an archive that was never stored.
    pub(crate) async fn put_object(&self, bucket: &str, key: &str, body: &[u8]) {
        // The create names its `LocationConstraint`: a create that names none can only
        // mean us-east-1, the Region the unspecified constraint stands for, so S3 refuses
        // one sent to any other Region's endpoint and this harness signs for
        // [`TEST_REGION`].
        let created = self
            .s3(
                "PUT",
                &format!("/{bucket}"),
                format!(
                    "<CreateBucketConfiguration \
                     xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
                     <LocationConstraint>{TEST_REGION}</LocationConstraint>\
                     </CreateBucketConfiguration>"
                )
                .as_bytes(),
            )
            .await;
        assert_eq!(
            created, 200,
            "seeding s3://{bucket}: expected 200 from CreateBucket, got {created}"
        );
        let stored = self.s3("PUT", &format!("/{bucket}/{key}"), body).await;
        assert_eq!(
            stored, 200,
            "seeding s3://{bucket}/{key}: expected 200 from PutObject, got {stored}"
        );
    }
}

/// One answer from the emulator: its status and its XML body.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: String,
}

impl Answer {
    fn of(response: http::Response<bytes::Bytes>) -> Self {
        let status = response.status().as_u16();
        let body = String::from_utf8_lossy(response.body().as_ref()).into_owned();
        Self { status, body }
    }

    /// Every value at `path` — a `/`-separated element path whose last segment
    /// is the element read and whose earlier segments are the enclosing
    /// elements, e.g. `Environments/member/EnvironmentId` — in document order.
    ///
    /// An element carrying no text is NOT reported: an absent member and one
    /// reported empty read the same way to almost every suite. The suite that
    /// asserts an EMPTY element is the exception and reads
    /// [`Answer::values_at_including_empty`] instead; both go through the one
    /// scanner below, so the two cannot disagree about where an element is.
    pub(crate) fn values_at(&self, path: &str) -> Vec<&str> {
        self.values_at_including_empty(path)
            .into_iter()
            .filter(|value| !value.trim().is_empty())
            .collect()
    }

    /// As [`Answer::values_at`], keeping the empty text an element with no
    /// content carries — for a member whose RECORDED value is the empty string,
    /// where "absent" and "present and empty" are different answers.
    pub(crate) fn values_at_including_empty(&self, path: &str) -> Vec<&str> {
        let mut segments = path.split('/').collect::<Vec<&str>>();
        let tag = segments.pop().expect("a path names at least one element");
        let mut scopes = vec![self.body.as_str()];
        for parent in segments {
            scopes = scopes
                .iter()
                .flat_map(|scope| elements(scope, parent))
                .collect();
        }
        scopes
            .iter()
            .flat_map(|scope| elements(scope, tag))
            .collect()
    }

    /// The first value at `path` (see [`Answer::values_at`]).
    pub(crate) fn value(&self, path: &str) -> Option<&str> {
        self.values_at(path).into_iter().next()
    }

    /// Every element at `path` as an [`Answer`] over its OWN inner XML, so a
    /// test reading several members of one list element keeps them together
    /// instead of re-aligning one flat list per member.
    pub(crate) fn elements_at(&self, path: &str) -> Vec<Answer> {
        self.values_at(path)
            .into_iter()
            .map(|inner| Answer {
                status: self.status,
                body: inner.to_string(),
            })
            .collect()
    }

    /// As [`Answer::value`], failing the named case when the element is absent.
    pub(crate) fn expect(&self, path: &str, case: &str) -> &str {
        self.value(path).unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a {path} element, got status {} and body {}",
                self.status, self.body
            )
        })
    }

    /// The awsQuery error envelope's `<Code>`, or `""` on a success envelope.
    pub(crate) fn error_code(&self) -> &str {
        self.value("Error/Code").unwrap_or_default()
    }

    /// The awsQuery error envelope's `<Message>`, or `""` on a success envelope,
    /// with the five predefined XML entities decoded — the envelope escapes the
    /// quotes an AWS validation message puts around a value, and a caller's SDK
    /// hands it back unescaped.
    pub(crate) fn error_message(&self) -> String {
        self.value("Error/Message")
            .unwrap_or_default()
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            // Last, so a decoded entity is not decoded a second time.
            .replace("&amp;", "&")
    }
}

/// The content of every `<tag>…</tag>` in `xml`, in document order.
///
/// Reads the slice between an opening `<tag>` — which may carry attributes, as
/// the first member of a list repeats the envelope's `xmlns` — and that
/// element's OWN `</tag>`, counting nested opens so a `<member>` that itself
/// contains a `<member>` (an application's `Versions` list inside an
/// application member) is read whole. For a leaf element that slice is its
/// text; for a wrapper it is its inner XML, so a path descends by calling this
/// once per segment. This service's serializer writes an empty member as
/// `<Tag></Tag>`, never `<Tag/>`, so there is no self-closing form to count.
pub(crate) fn elements<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let close = format!("</{tag}>");
    let mut found = Vec::new();
    let mut from = 0usize;
    while let Some((_, content)) = opening_tag(xml, tag, from) {
        let mut cursor = content;
        let mut depth = 1usize;
        let mut end = None;
        while depth > 0 {
            let next_open = opening_tag(xml, tag, cursor);
            let next_close = xml[cursor..].find(&close).map(|at| cursor + at);
            match (next_open, next_close) {
                (Some((opened, after)), Some(closed)) if opened < closed => {
                    depth += 1;
                    cursor = after;
                }
                (_, Some(closed)) => {
                    depth -= 1;
                    cursor = closed + close.len();
                    if depth == 0 {
                        end = Some(closed);
                    }
                }
                // An element that opened and never closed: not a document this
                // service emits, so stop rather than guess.
                (_, None) => break,
            }
        }
        let Some(end) = end else {
            break;
        };
        found.push(&xml[content..end]);
        from = cursor;
    }
    found
}

/// The next opening tag for `tag` at or after `from`, as
/// `(start of '<', start of the content)`. Accepts attributes on the element and
/// skips a longer element name that merely starts with `tag`.
fn opening_tag(xml: &str, tag: &str, from: usize) -> Option<(usize, usize)> {
    let needle = format!("<{tag}");
    let mut at = from;
    loop {
        let start = at + xml[at..].find(&needle)?;
        let after = start + needle.len();
        match xml[after..].chars().next() {
            Some('>') => return Some((start, after + 1)),
            Some(character) if character.is_whitespace() => {
                let closed = after + xml[after..].find('>')?;
                return Some((start, closed + 1));
            }
            Some(_) => at = after,
            None => return None,
        }
    }
}

/// Percent-encode a form value: everything outside SigV4's unreserved set
/// (`A-Z a-z 0-9 - _ . ~`) becomes `%XX`, which is how an AWS SDK encodes an
/// awsQuery parameter and what this service's query deserializer decodes.
///
/// `pub(crate)` so a suite that drives ANOTHER awsQuery service mounted beside
/// this one (SNS, in the environment-notification suite) encodes its form with
/// the same rule instead of keeping a second copy of it.
pub(crate) fn form_encoded(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(byte));
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// `e-` followed by exactly [`eb::state::ENVIRONMENT_ID_BODY_LENGTH`] lowercase
/// alphanumerics, the form the real service issues an `EnvironmentId` in.
pub(crate) fn is_environment_id(id: &str) -> bool {
    id.strip_prefix("e-").is_some_and(|body| {
        body.len() == eb::state::ENVIRONMENT_ID_BODY_LENGTH
            && body
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    })
}

pub(crate) fn account() -> &'static str {
    engine::emulator_account_id()
}

/// Hold `cname` to the FORM an environment's published `CNAME` carries, for the
/// environment holding the CNAME prefix `prefix` in `region`, and answer the
/// environment's own sub-domain label out of it.
///
/// The form is spelled out here rather than read off the crate, so every suite
/// holds the service to the same one: a dot-separated DNS name carrying no port
/// and no path, whose FIRST label is the prefix as ONE DNS label — the published
/// value is the authority of an address a client dials, so the prefix reaches it
/// folded through the workspace's own reading of that question
/// (`service_common::data_plane_addressing::dns_label`), which is the documented
/// route back to it since DNS compares labels case-insensitively — then one label
/// the service minted for this environment alone, then the Region, then this
/// service's own label in the zone this emulator announces its addresses in
/// ([`PUBLISHED_DOMAIN`]).
pub(crate) fn assert_published_cname<'c>(
    case: &str,
    cname: &'c str,
    region: &str,
    prefix: &str,
) -> &'c str {
    assert!(
        !cname.contains(':') && !cname.contains('/'),
        "case {case:?}: a published CNAME carries neither ':' nor '/', got {cname:?}"
    );
    let labels: Vec<&str> = cname.split('.').collect();
    assert!(
        labels.iter().all(|label| !label.is_empty()),
        "case {case:?}: a published CNAME carries no empty label, got {cname:?}"
    );
    assert_eq!(
        labels.first().copied(),
        Some(service_common::data_plane_addressing::dns_label(prefix).as_str()),
        "case {case:?}: the first label must be the environment's CNAME prefix as one DNS \
         label, got {cname:?}"
    );
    assert_eq!(
        labels.get(2).copied(),
        Some(region),
        "case {case:?}: the Region must be the label behind the sub-domain, got {cname:?}"
    );
    assert!(
        cname.ends_with(&format!(".{PUBLISHED_DOMAIN}")),
        "case {case:?}: expected an address under {PUBLISHED_DOMAIN}, got {cname:?}"
    );
    assert_eq!(
        labels.len(),
        3 + PUBLISHED_DOMAIN.split('.').count(),
        "case {case:?}: expected the prefix, one sub-domain, the Region and this service's \
         own domain, got {cname:?}"
    );
    labels
        .get(1)
        .copied()
        .unwrap_or_else(|| panic!("case {case:?}: expected a sub-domain label in {cname:?}"))
}

/// A timestamp this service rendered, as the INSTANT it names.
///
/// Two timestamps are ordered as instants, never as text: the `date-time` rendering
/// the query protocol uses carries a variable-width fractional second, so `Z` can
/// land where a digit lands in the other string (`"…:05.2Z"` vs `"…:05.25Z"`, where
/// `Z` sorts AFTER `5`) and a lexicographic comparison of two correctly ordered
/// timestamps then fails. Shared, so every suite of this target orders a timestamp
/// the one way.
pub(crate) fn instant(rendered: &str, case: &str) -> f64 {
    ::aws_smithy_types::DateTime::from_str(
        rendered,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| {
        panic!("case {case:?}: expected an ISO-8601 timestamp, got {rendered:?}: {error}")
    })
    .as_secs_f64()
}

/// How long a status change the clock owes is waited for, and how often
/// [`wait_for_status`] looks again. The deadline is GENEROUS — it bounds nothing
/// any suite asserts, it only stops an environment that never settles from
/// hanging the target.
pub(crate) const STATUS_DEADLINE: Duration = Duration::from_secs(120);
/// See [`STATUS_DEADLINE`].
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Look again until the environment `name` reports `expected` — the CONDITION the
/// caller waits on, never a fixed wait for the clock — or until
/// [`STATUS_DEADLINE`] has passed.
///
/// Deleted records are included in the report, so a wait for the end of a
/// lifecycle sees the status it is waiting for. Shared by every suite that waits
/// on a transition, so none of them keeps its own copy of the poll.
pub(crate) async fn wait_for_status(emulator: &Emulator, case: &str, name: &str, expected: &str) {
    let started = std::time::Instant::now();
    loop {
        let described = emulator
            .call(
                "DescribeEnvironments",
                &[
                    ("EnvironmentNames.member.1", name),
                    ("IncludeDeleted", "true"),
                ],
            )
            .await;
        let status = described
            .values_at("Environments/member/Status")
            .first()
            .map(|status| (*status).to_string())
            .unwrap_or_default();
        if status == expected {
            return;
        }
        assert!(
            started.elapsed() < STATUS_DEADLINE,
            "case {case:?}: environment {name:?} never reached {expected}, last status \
             {status:?} in body {}",
            described.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Look again until the version `version_label` of `application` reports
/// `expected` — the CONDITION a client that waits for a version to become
/// deployable polls for, never a fixed wait for the clock — or until
/// [`STATUS_DEADLINE`] has passed, and hand back the report it settled in.
///
/// `expected` is a modelled `ApplicationVersionStatus`; what a reader puts on the
/// wire is the upper-cased form of it, which is what this compares. Shared by
/// every suite that waits on a version's pre-processing, so none of them keeps
/// its own copy of the poll — the deadline and the interval are stated once,
/// here, beside the environment poll above.
pub(crate) async fn wait_for_version_status(
    emulator: &Emulator,
    case: &str,
    application: &str,
    version_label: &str,
    expected: &str,
) -> Answer {
    let expected = eb::state::version_status_on_the_wire(expected);
    let started = std::time::Instant::now();
    loop {
        let described = emulator
            .call(
                "DescribeApplicationVersions",
                &[
                    ("ApplicationName", application),
                    ("VersionLabels.member.1", version_label),
                ],
            )
            .await;
        let status = described
            .value("ApplicationVersions/member/Status")
            .unwrap_or_default()
            .to_string();
        if status == expected {
            return described;
        }
        assert!(
            started.elapsed() < STATUS_DEADLINE,
            "case {case:?}: version {version_label:?} of {application:?} never reached \
             {expected}, last status {status:?} in body {}",
            described.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

// ---------------------------------------------------------------------------
// The lifecycle, end to end
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_application_and_environment_lifecycle_runs_end_to_end() {
    let emulator = Emulator::new();
    let case = "lifecycle";

    // 1. CreateApplication answers with what it stored.
    let created = emulator
        .call(
            "CreateApplication",
            &[("ApplicationName", "my-app"), ("Description", "demo")],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreateApplication, got {} and body {}",
        created.status, created.body
    );
    assert_eq!(
        created.expect("Application/ApplicationName", case),
        "my-app",
        "case {case:?}: CreateApplication must answer with the stored name, got {}",
        created.body
    );
    assert_eq!(
        created.expect("Application/ApplicationArn", case),
        format!(
            "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:application/my-app",
            account()
        ),
        "case {case:?}: unexpected ApplicationArn in {}",
        created.body
    );
    assert_eq!(
        created.expect("Application/Description", case),
        "demo",
        "case {case:?}: unexpected Description in {}",
        created.body
    );
    assert!(
        created.value("Application/DateCreated").is_some(),
        "case {case:?}: expected a DateCreated, got {}",
        created.body
    );

    // 2. CreateApplicationVersion stores the version under that application.
    let version = emulator
        .call(
            "CreateApplicationVersion",
            &[
                ("ApplicationName", "my-app"),
                ("VersionLabel", "v1"),
                ("SourceBundle.S3Bucket", "my-bucket"),
                ("SourceBundle.S3Key", "app.zip"),
            ],
        )
        .await;
    assert_eq!(
        version.status, 200,
        "case {case:?}: expected 200 from CreateApplicationVersion, got {} and body {}",
        version.status, version.body
    );
    assert_eq!(
        version.expect("ApplicationVersion/VersionLabel", case),
        "v1",
        "case {case:?}: unexpected VersionLabel in {}",
        version.body
    );
    assert_eq!(
        version.expect("ApplicationVersion/Status", case),
        "UNPROCESSED",
        "case {case:?}: a version nothing processed must be UNPROCESSED, got {}",
        version.body
    );
    assert_eq!(
        version.expect("ApplicationVersion/ApplicationVersionArn", case),
        format!(
            "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:applicationversion/my-app/v1",
            account()
        ),
        "case {case:?}: unexpected ApplicationVersionArn in {}",
        version.body
    );
    assert_eq!(
        version.expect("SourceBundle/S3Key", case),
        "app.zip",
        "case {case:?}: SourceBundle must come back from storage, got {}",
        version.body
    );

    // 3. DescribeApplications reports the application with its version.
    let applications = emulator.call("DescribeApplications", &[]).await;
    assert_eq!(
        applications.values_at("Applications/member/ApplicationName"),
        vec!["my-app"],
        "case {case:?}: unexpected applications in {}",
        applications.body
    );
    assert_eq!(
        applications.values_at("Versions/member"),
        vec!["v1"],
        "case {case:?}: the new version must appear in the application's Versions, got {}",
        applications.body
    );

    // 4. CreateEnvironment mints and persists the environment.
    let environment = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", "my-app"),
                ("EnvironmentName", "my-env-1"),
                ("VersionLabel", "v1"),
                (
                    "SolutionStackName",
                    SOLUTION_STACK.as_str(),
                ),
            ],
        )
        .await;
    assert_eq!(
        environment.status, 200,
        "case {case:?}: expected 200 from CreateEnvironment, got {} and body {}",
        environment.status, environment.body
    );
    let environment_id = environment.expect("EnvironmentId", case).to_string();
    assert!(
        is_environment_id(&environment_id),
        "case {case:?}: expected an e-<lowercase alnum> EnvironmentId of the published length, \
         got {environment_id:?}"
    );
    // The launch has not been assigned its address yet, so it reports none: the
    // environment is assigned the name its launch reserved once that
    // provisioning finishes, which is where the CNAME is read below.
    assert!(
        environment.value("CNAME").is_none(),
        "case {case:?}: a launch still provisioning reports no CNAME, got {}",
        environment.body
    );
    assert_eq!(
        environment.expect("Status", case),
        "Launching",
        "case {case:?}: a new environment reports Launching, got {}",
        environment.body
    );
    assert_eq!(
        environment.expect("Health", case),
        "Grey",
        "case {case:?}: a new environment reports Grey health, got {}",
        environment.body
    );
    assert_eq!(
        environment.expect("HealthStatus", case),
        "Pending",
        "case {case:?}: a new environment reports Pending health status, got {}",
        environment.body
    );
    assert_eq!(
        environment.expect("EnvironmentArn", case),
        format!(
            "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:environment/my-app/my-env-1",
            account()
        ),
        "case {case:?}: unexpected EnvironmentArn in {}",
        environment.body
    );

    // 5. DescribeEnvironments returns exactly what was persisted — once the
    //    provisioning the launch started has finished, which is when the
    //    environment is assigned the CNAME its launch reserved.
    wait_for_status(&emulator, case, "my-env-1", "Ready").await;
    let described = emulator
        .call("DescribeEnvironments", &[("ApplicationName", "my-app")])
        .await;
    assert_eq!(
        described.values_at("Environments/member/EnvironmentId"),
        vec![environment_id.as_str()],
        "case {case:?}: DescribeEnvironments must return the created environment, got {}",
        described.body
    );
    let cname = described
        .expect("Environments/member/CNAME", case)
        .to_string();
    assert!(
        cname.starts_with("my-env-1-"),
        "case {case:?}: the assigned CNAME must carry the generated prefix of this \
         environment, got {cname:?}"
    );
    assert_eq!(
        described.values_at("Environments/member/CNAME"),
        vec![cname.as_str()],
        "case {case:?}: the CNAME must survive the round trip, got {}",
        described.body
    );
    assert_eq!(
        described.values_at("Environments/member/VersionLabel"),
        vec!["v1"],
        "case {case:?}: the VersionLabel must survive the round trip, got {}",
        described.body
    );

    // 6. TerminateEnvironment moves it to Terminating, and the record leaves the
    //    default view once the clock completes the termination.
    let terminated = emulator
        .call(
            "TerminateEnvironment",
            &[("EnvironmentName", "my-env-1")],
        )
        .await;
    assert_eq!(
        terminated.status, 200,
        "case {case:?}: expected 200 from TerminateEnvironment, got {} and body {}",
        terminated.status, terminated.body
    );
    assert_eq!(
        terminated.expect("Status", case),
        "Terminating",
        "case {case:?}: TerminateEnvironment reports Terminating, got {}",
        terminated.body
    );
    // The termination the clock owes is waited for as the CONDITION the
    // environment reaches, never as a fixed time.
    wait_for_status(&emulator, case, "my-env-1", "Terminated").await;

    let after = emulator.call("DescribeEnvironments", &[]).await;
    assert!(
        after.values_at("Environments/member/EnvironmentId").is_empty(),
        "case {case:?}: a terminated environment must not be listed, got {}",
        after.body
    );
    let with_deleted = emulator
        .call("DescribeEnvironments", &[("IncludeDeleted", "true")])
        .await;
    assert_eq!(
        with_deleted.values_at("Environments/member/EnvironmentId"),
        vec![environment_id.as_str()],
        "case {case:?}: IncludeDeleted must list the terminated environment, got {}",
        with_deleted.body
    );
    assert_eq!(
        with_deleted.values_at("Environments/member/Status"),
        vec!["Terminated"],
        "case {case:?}: the environment must have reached Terminated, got {}",
        with_deleted.body
    );

    // 7. DeleteApplication starts the application's deletion. The delete is
    // asynchronous, so the application is still described until the clock has
    // made its removal due.
    let deleted = emulator
        .call(
            "DeleteApplication",
            &[
                ("ApplicationName", "my-app"),
                ("TerminateEnvByForce", "true"),
            ],
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case {case:?}: expected 200 from DeleteApplication, got {} and body {}",
        deleted.status, deleted.body
    );
    let deleting = emulator.call("DescribeApplications", &[]).await;
    assert_eq!(
        deleting.values_at("Applications/member/ApplicationName"),
        vec!["my-app"],
        "case {case:?}: an application whose deletion is still running must still be \
         described, got {}",
        deleting.body
    );
    // The application's removal is waited for as the CONDITION a client polling
    // the report sees, never as a fixed time.
    let started = std::time::Instant::now();
    let gone = loop {
        let described = emulator.call("DescribeApplications", &[]).await;
        if described
            .values_at("Applications/member/ApplicationName")
            .is_empty()
        {
            break described;
        }
        assert!(
            started.elapsed() < STATUS_DEADLINE,
            "case {case:?}: the deleted application was still listed, got {}",
            described.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    assert!(
        gone.values_at("Applications/member/ApplicationName").is_empty(),
        "case {case:?}: the deleted application must not be listed, got {}",
        gone.body
    );
}

// ---------------------------------------------------------------------------
// Stored values come back as stored
// ---------------------------------------------------------------------------

/// One row per application shape the create/describe pair must round-trip.
struct ApplicationCase {
    name: &'static str,
    description: Option<&'static str>,
    /// Version labels to create, oldest first.
    versions: &'static [&'static str],
}

#[tokio::test]
async fn described_applications_are_the_stored_applications() {
    // Rows differ along what the rule generalises over: name length and shape,
    // whether a description was supplied, and how many versions the application
    // accumulated.
    let cases = [
        ApplicationCase {
            name: "a",
            description: None,
            versions: &[],
        },
        ApplicationCase {
            name: "app-with-dashes-and-digits-0123",
            description: Some("a description"),
            versions: &["v1"],
        },
        ApplicationCase {
            // The modelled maximum for `ApplicationName` is 100 characters.
            name: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            description: Some(""),
            versions: &["1.0.0", "1.0.1", "release-2"],
        },
    ];

    let emulator = Emulator::new();
    for case in &cases {
        let mut params: Vec<(&str, &str)> = vec![("ApplicationName", case.name)];
        if let Some(description) = case.description {
            params.push(("Description", description));
        }
        let created = emulator.call("CreateApplication", &params).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected 200 from CreateApplication, got {} and body {}",
            case.name, created.status, created.body
        );

        for label in case.versions {
            let version = emulator
                .call(
                    "CreateApplicationVersion",
                    &[("ApplicationName", case.name), ("VersionLabel", label)],
                )
                .await;
            assert_eq!(
                version.status, 200,
                "case {:?}: expected 200 from CreateApplicationVersion for {label:?}, got {} and body {}",
                case.name, version.status, version.body
            );
        }
    }

    for case in &cases {
        let described = emulator
            .call("DescribeApplications", &[("ApplicationNames.member.1", case.name)])
            .await;
        assert_eq!(
            described.values_at("Applications/member/ApplicationName"),
            vec![case.name],
            "case {:?}: the name filter must return exactly that application, got {}",
            case.name,
            described.body
        );
        assert_eq!(
            described.value("Applications/member/Description"),
            // An empty description is stored, and an empty XML element carries no
            // text node, so it reads back as absent rather than as "".
            case.description.filter(|text| !text.is_empty()),
            "case {:?}: unexpected Description in {}",
            case.name,
            described.body
        );
        // "Versions: The names of the versions for this application", newest first.
        let expected: Vec<&str> = case.versions.iter().rev().copied().collect();
        assert_eq!(
            described.values_at("Versions/member"),
            expected,
            "case {:?}: expected the accumulated versions newest first, got {}",
            case.name,
            described.body
        );
    }
}

/// One row per environment shape CreateEnvironment must derive and persist.
struct EnvironmentCase {
    environment_name: &'static str,
    cname_prefix: Option<&'static str>,
    tier_name: Option<&'static str>,
    tier_type: Option<&'static str>,
    region: &'static str,
}

#[tokio::test]
async fn created_environments_are_derived_from_the_request_and_persisted() {
    // Rows differ along the inputs the derivations read: whether a CNAME prefix
    // was supplied, which tier was asked for, and which region signed the call.
    let cases = [
        EnvironmentCase {
            environment_name: "env-default",
            cname_prefix: None,
            tier_name: None,
            tier_type: None,
            region: TEST_REGION,
        },
        EnvironmentCase {
            environment_name: "env-with-prefix",
            cname_prefix: Some("chosen-prefix"),
            tier_name: None,
            tier_type: None,
            region: TEST_REGION,
        },
        EnvironmentCase {
            // "For Standard-mode EC2-based backend application with Amazon SQS –
            // Worker", with "For Worker tier – SQS/HTTP" as its type.
            environment_name: "env-worker",
            cname_prefix: None,
            tier_name: Some("Worker"),
            tier_type: Some("SQS/HTTP"),
            region: "eu-central-1",
        },
    ];

    let emulator = Emulator::new();
    let created_application = emulator
        .call("CreateApplication", &[("ApplicationName", "tiers")])
        .await;
    assert_eq!(
        created_application.status, 200,
        "expected 200 from CreateApplication, got {} and body {}",
        created_application.status, created_application.body
    );

    for case in &cases {
        let name = case.environment_name;
        let mut params: Vec<(&str, &str)> = vec![
            ("ApplicationName", "tiers"),
            ("EnvironmentName", name),
            ("SolutionStackName", SOLUTION_STACK.as_str()),
        ];
        if let Some(prefix) = case.cname_prefix {
            params.push(("CNAMEPrefix", prefix));
        }
        if let Some(tier) = case.tier_name {
            params.push(("Tier.Name", tier));
        }
        if let Some(tier_type) = case.tier_type {
            params.push(("Tier.Type", tier_type));
        }
        let created = emulator
            .call_in_region(case.region, "CreateEnvironment", &params)
            .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: expected 200 from CreateEnvironment, got {} and body {}",
            created.status, created.body
        );

        let environment_id = created.expect("EnvironmentId", name).to_string();
        // The launch reports no address yet: the environment is assigned the
        // name its launch reserved when the provisioning finishes, which is
        // where the CNAME is read below.
        assert!(
            created.value("CNAME").is_none(),
            "case {name:?}: a launch still provisioning reports no CNAME, got {}",
            created.body
        );
        let expected_prefix = case
            .cname_prefix
            .map(str::to_string)
            // With no prefix the environment name carries one appended
            // alphanumeric string — its own unique id body.
            .unwrap_or_else(|| format!("{name}-{}", environment_id.trim_start_matches("e-")));
        assert_eq!(
            created.expect("EnvironmentArn", name),
            format!(
                "arn:aws:elasticbeanstalk:{}:{}:environment/tiers/{name}",
                case.region,
                account()
            ),
            "case {name:?}: the ARN must carry the signing region, got {}",
            created.body
        );
        assert_eq!(
            created.expect("Tier/Name", name),
            case.tier_name.unwrap_or("WebServer"),
            "case {name:?}: unexpected Tier.Name in {}",
            created.body
        );
        assert_eq!(
            created.expect("Tier/Type", name),
            case.tier_type.unwrap_or("Standard"),
            "case {name:?}: unexpected Tier.Type in {}",
            created.body
        );

        // The round trip: everything above must come back from the store, with
        // the address the environment is assigned once its launch has finished.
        wait_for_status(&emulator, name, name, "Ready").await;
        let described = emulator
            .call(
                "DescribeEnvironments",
                &[("EnvironmentIds.member.1", environment_id.as_str())],
            )
            .await;
        assert_eq!(
            described.values_at("Environments/member/EnvironmentName"),
            vec![name],
            "case {name:?}: the id filter must return that environment, got {}",
            described.body
        );
        let cname = described
            .expect("Environments/member/CNAME", name)
            .to_string();
        assert_published_cname(name, &cname, case.region, &expected_prefix);
        assert_eq!(
            described.values_at("Environments/member/Tier/Name"),
            vec![case.tier_name.unwrap_or("WebServer")],
            "case {name:?}: the stored tier must come back, got {}",
            described.body
        );
    }

    // Every environment is distinct in the store, each with its own id.
    let all = emulator
        .call("DescribeEnvironments", &[("ApplicationName", "tiers")])
        .await;
    let ids = all.values_at("Environments/member/EnvironmentId");
    assert_eq!(
        ids.len(),
        cases.len(),
        "expected one environment per case, got {}",
        all.body
    );
    let mut unique: Vec<&str> = ids.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        ids.len(),
        "EnvironmentIds must be unique, got {ids:?}"
    );
}

// ---------------------------------------------------------------------------
// Filtering and pagination
// ---------------------------------------------------------------------------

#[tokio::test]
async fn describe_environments_filters_and_paginates() {
    let emulator = Emulator::new();
    for application in ["left", "right"] {
        assert_eq!(
            emulator
                .call("CreateApplication", &[("ApplicationName", application)])
                .await
                .status,
            200,
            "expected 200 from CreateApplication for {application:?}"
        );
        assert_eq!(
            emulator
                .call(
                    "CreateApplicationVersion",
                    &[("ApplicationName", application), ("VersionLabel", "v1")],
                )
                .await
                .status,
            200,
            "expected 200 from CreateApplicationVersion for {application:?}"
        );
    }
    let mut ids = Vec::new();
    for (application, environment) in [
        ("left", "left-env-1"),
        ("left", "left-env-2"),
        ("right", "right-env-1"),
    ] {
        let created = emulator
            .call(
                "CreateEnvironment",
                &[
                    ("ApplicationName", application),
                    ("EnvironmentName", environment),
                    ("SolutionStackName", SOLUTION_STACK.as_str()),
                    ("VersionLabel", "v1"),
                ],
            )
            .await;
        assert_eq!(
            created.status, 200,
            "expected 200 from CreateEnvironment for {environment:?}, got {}",
            created.body
        );
        ids.push(created.expect("EnvironmentId", environment).to_string());
    }

    // One row per filter the operation declares.
    struct FilterCase<'a> {
        case: &'a str,
        params: Vec<(&'a str, &'a str)>,
        expected: Vec<&'a str>,
    }
    let filters = [
        FilterCase {
            case: "no filter",
            params: vec![],
            expected: vec!["right-env-1", "left-env-2", "left-env-1"],
        },
        FilterCase {
            case: "ApplicationName",
            params: vec![("ApplicationName", "left")],
            expected: vec!["left-env-2", "left-env-1"],
        },
        FilterCase {
            case: "EnvironmentNames",
            params: vec![("EnvironmentNames.member.1", "right-env-1")],
            expected: vec!["right-env-1"],
        },
        FilterCase {
            case: "EnvironmentIds",
            params: vec![("EnvironmentIds.member.1", ids[1].as_str())],
            expected: vec!["left-env-2"],
        },
        FilterCase {
            case: "VersionLabel that nothing is deployed from",
            params: vec![("VersionLabel", "v9")],
            expected: vec![],
        },
    ];
    for FilterCase {
        case,
        params,
        expected,
    } in &filters
    {
        let described = emulator.call("DescribeEnvironments", params).await;
        assert_eq!(
            described.values_at("Environments/member/EnvironmentName"),
            *expected,
            "case {case:?}: unexpected environments in {}",
            described.body
        );
    }

    // MaxRecords pages, and NextToken resumes where the page ended.
    let first = emulator
        .call("DescribeEnvironments", &[("MaxRecords", "2")])
        .await;
    assert_eq!(
        first.values_at("Environments/member/EnvironmentName"),
        vec!["right-env-1", "left-env-2"],
        "case \"MaxRecords=2\": unexpected first page in {}",
        first.body
    );
    let token = first
        .value("NextToken")
        .unwrap_or_else(|| panic!("case \"MaxRecords=2\": expected a NextToken in {}", first.body))
        .to_string();
    let second = emulator
        .call(
            "DescribeEnvironments",
            &[("MaxRecords", "2"), ("NextToken", token.as_str())],
        )
        .await;
    assert_eq!(
        second.values_at("Environments/member/EnvironmentName"),
        vec!["left-env-1"],
        "case \"page 2\": unexpected second page in {}",
        second.body
    );
    assert_eq!(
        second.value("NextToken"),
        None,
        "case \"page 2\": the last page carries no NextToken, got {}",
        second.body
    );
}

// ---------------------------------------------------------------------------
// The clock-owed status transitions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_launching_environment_becomes_ready_when_the_clock_makes_it_due() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator
            .call("CreateApplication", &[("ApplicationName", "clock")])
            .await
            .status,
        200,
        "expected 200 from CreateApplication"
    );
    let created = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", "clock"),
                ("EnvironmentName", "clock-env"),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from CreateEnvironment, got {}",
        created.body
    );

    // A caller acting straight after the call still sees the transient status.
    let immediately = emulator.call("DescribeEnvironments", &[]).await;
    assert_eq!(
        immediately.values_at("Environments/member/Status"),
        vec!["Launching"],
        "case \"immediately\": a launch in progress must still report Launching, got {}",
        immediately.body
    );

    std::thread::sleep(PAST_DUE);
    let later = emulator.call("DescribeEnvironments", &[]).await;
    assert_eq!(
        later.values_at("Environments/member/Status"),
        vec!["Ready"],
        "case \"past due\": the launch the clock owed must have completed, got {}",
        later.body
    );
    assert_eq!(
        later.values_at("Environments/member/Health"),
        vec!["Green"],
        "case \"past due\": a launched environment reports Green, got {}",
        later.body
    );
    assert_eq!(
        later.values_at("Environments/member/HealthStatus"),
        vec!["Ok"],
        "case \"past due\": a launched environment reports Ok, got {}",
        later.body
    );

    // A describe does not move the environment on by itself: reading it again
    // reports the same status.
    let again = emulator.call("DescribeEnvironments", &[]).await;
    assert_eq!(
        again.values_at("Environments/member/Status"),
        vec!["Ready"],
        "case \"read twice\": a read must not change what a later read sees, got {}",
        again.body
    );

    // And the name of a terminated environment can be used again.
    assert_eq!(
        emulator
            .call("TerminateEnvironment", &[("EnvironmentName", "clock-env")])
            .await
            .status,
        200,
        "expected 200 from TerminateEnvironment"
    );
    std::thread::sleep(PAST_DUE);
    let reused = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", "clock"),
                ("EnvironmentName", "clock-env"),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
            ],
        )
        .await;
    assert_eq!(
        reused.status, 200,
        "case \"name reuse\": a terminated environment no longer holds its name, got {} and body {}",
        reused.status, reused.body
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// One row per request the service must refuse, with the code it refuses with.
struct RefusalCase {
    case: &'static str,
    action: &'static str,
    params: Vec<(&'static str, &'static str)>,
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn bad_requests_are_refused_with_the_services_own_error_codes() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator
            .call("CreateApplication", &[("ApplicationName", "taken")])
            .await
            .status,
        200,
        "expected 200 from CreateApplication"
    );
    assert_eq!(
        emulator
            .call(
                "CreateApplicationVersion",
                &[("ApplicationName", "taken"), ("VersionLabel", "v1")],
            )
            .await
            .status,
        200,
        "expected 200 from CreateApplicationVersion"
    );
    assert_eq!(
        emulator
            .call(
                "CreateEnvironment",
                &[
                    ("ApplicationName", "taken"),
                    ("EnvironmentName", "taken-env"),
                    ("SolutionStackName", SOLUTION_STACK.as_str()),
                ],
            )
            .await
            .status,
        200,
        "expected 200 from CreateEnvironment"
    );

    // Rows span the classes of refusal: a name that is already used, a name that
    // names nothing, an absent either/or parameter, an absent required member,
    // and a value outside a modelled bound. A value the MODEL refuses is a
    // `ValidationError` (the request is malformed); a value the STORE refuses is
    // an `InvalidParameterValue` (the request is well formed and names something
    // that cannot be used). Deleting an application that does not exist is in
    // neither class — see `request_refusals.rs`.
    let cases = vec![
        RefusalCase {
            case: "duplicate application",
            action: "CreateApplication",
            params: vec![("ApplicationName", "taken")],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "duplicate version",
            action: "CreateApplicationVersion",
            params: vec![("ApplicationName", "taken"), ("VersionLabel", "v1")],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "duplicate environment name",
            action: "CreateEnvironment",
            params: vec![
                ("ApplicationName", "taken"),
                ("EnvironmentName", "taken-env"),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
            ],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "version for an unknown application",
            action: "CreateApplicationVersion",
            params: vec![("ApplicationName", "nope"), ("VersionLabel", "v1")],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "environment for an unknown application",
            action: "CreateEnvironment",
            params: vec![
                ("ApplicationName", "nope"),
                ("EnvironmentName", "nope-env"),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
            ],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "environment from an unknown version",
            action: "CreateEnvironment",
            params: vec![
                ("ApplicationName", "taken"),
                ("EnvironmentName", "other-env"),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
                ("VersionLabel", "v9"),
            ],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "terminate an unknown environment",
            action: "TerminateEnvironment",
            params: vec![("EnvironmentName", "ghost-env")],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "terminate with neither id nor name",
            action: "TerminateEnvironment",
            params: vec![],
            status: 400,
            code: "MissingRequiredParameter",
        },
        RefusalCase {
            case: "create an environment with no name",
            action: "CreateEnvironment",
            params: vec![
                ("ApplicationName", "taken"),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
            ],
            status: 400,
            code: "MissingRequiredParameter",
        },
        RefusalCase {
            case: "absent required ApplicationName",
            action: "CreateApplication",
            params: vec![("Description", "no name supplied")],
            status: 400,
            code: "MissingRequiredParameter",
        },
        RefusalCase {
            case: "EnvironmentName under the modelled minimum of 4",
            action: "CreateEnvironment",
            params: vec![("ApplicationName", "taken"), ("EnvironmentName", "ab")],
            status: 400,
            code: "ValidationError",
        },
        RefusalCase {
            // The modelled maximum for `ApplicationName` is 100 characters.
            case: "ApplicationName over the modelled maximum of 100",
            action: "CreateApplication",
            params: vec![(
                "ApplicationName",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )],
            status: 400,
            code: "ValidationError",
        },
        RefusalCase {
            case: "MaxRecords over the modelled maximum of 1000",
            action: "DescribeEnvironments",
            params: vec![("MaxRecords", "1001")],
            status: 400,
            code: "ValidationError",
        },
    ];

    for RefusalCase {
        case,
        action,
        params,
        status,
        code,
    } in &cases
    {
        let refused = emulator.call(action, params).await;
        assert_eq!(
            refused.status, *status,
            "case {case:?}: expected HTTP {status}, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            *code,
            "case {case:?}: expected error code {code}, got body {}",
            refused.body
        );
        assert!(
            refused.value("Error/Message").is_some(),
            "case {case:?}: a refusal must say what to fix, got body {}",
            refused.body
        );
    }
}

#[tokio::test]
async fn an_application_with_running_environments_is_not_deleted_silently() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator
            .call("CreateApplication", &[("ApplicationName", "busy")])
            .await
            .status,
        200,
        "expected 200 from CreateApplication"
    );
    let created = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", "busy"),
                ("EnvironmentName", "busy-env"),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from CreateEnvironment, got {}",
        created.body
    );

    // "You cannot delete an application that has a running environment."
    let refused = emulator
        .call("DeleteApplication", &[("ApplicationName", "busy")])
        .await;
    assert_eq!(
        refused.status, 400,
        "case \"running environment\": expected HTTP 400, got {} and body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "InvalidParameterValue",
        "case \"running environment\": unexpected error code in {}",
        refused.body
    );
    assert_eq!(
        refused.error_message(),
        "Unable to delete application busy because it has a version that is deployed to a \
         running environment.",
        "case \"running environment\": unexpected refusal message in {}",
        refused.body
    );
    let still_there = emulator.call("DescribeApplications", &[]).await;
    assert_eq!(
        still_there.values_at("Applications/member/ApplicationName"),
        vec!["busy"],
        "case \"running environment\": the refused delete must not have removed it, got {}",
        still_there.body
    );

    // "TerminateEnvByForce: … running environments will be terminated before
    // deleting the application."
    let deleted = emulator
        .call(
            "DeleteApplication",
            &[
                ("ApplicationName", "busy"),
                ("TerminateEnvByForce", "true"),
            ],
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case \"forced\": expected HTTP 200, got {} and body {}",
        deleted.status, deleted.body
    );
    let terminating = emulator.call("DescribeEnvironments", &[]).await;
    assert_eq!(
        terminating.values_at("Environments/member/Status"),
        vec!["Terminating"],
        "case \"forced\": the environment must be shutting down, got {}",
        terminating.body
    );
    wait_for_status(&emulator, "forced", "busy-env", "Terminated").await;
    let gone = emulator.call("DescribeEnvironments", &[]).await;
    assert!(
        gone.values_at("Environments/member/EnvironmentId").is_empty(),
        "case \"forced\": the environment must have reached Terminated, got {}",
        gone.body
    );
    let applications = emulator.call("DescribeApplications", &[]).await;
    assert!(
        applications
            .values_at("Applications/member/ApplicationName")
            .is_empty(),
        "case \"forced\": the application must be gone, got {}",
        applications.body
    );
}

#[tokio::test]
async fn auto_create_application_creates_the_application_it_names() {
    let emulator = Emulator::new();
    let created = emulator
        .call(
            "CreateApplicationVersion",
            &[
                ("ApplicationName", "auto-created"),
                ("VersionLabel", "v1"),
                ("AutoCreateApplication", "true"),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case \"AutoCreateApplication\": expected 200, got {} and body {}",
        created.status, created.body
    );
    let applications = emulator.call("DescribeApplications", &[]).await;
    assert_eq!(
        applications.values_at("Applications/member/ApplicationName"),
        vec!["auto-created"],
        "case \"AutoCreateApplication\": the application must have been created, got {}",
        applications.body
    );
    assert_eq!(
        applications.values_at("Versions/member"),
        vec!["v1"],
        "case \"AutoCreateApplication\": the version must belong to it, got {}",
        applications.body
    );
}

// ---------------------------------------------------------------------------
// The account quotas, which are also what bounds the store
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_account_quotas_are_enforced_with_the_modelled_errors() {
    let emulator = Emulator::new();

    // Applications: the quota is reached, and the next create is refused with
    // the error the model declares for it.
    for index in 0..eb::state::MAX_APPLICATIONS {
        let name = format!("app-{index:04}");
        let created = emulator
            .call("CreateApplication", &[("ApplicationName", name.as_str())])
            .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: expected 200 while filling the application quota, got {} and body {}",
            created.status, created.body
        );
    }
    let over = emulator
        .call("CreateApplication", &[("ApplicationName", "one-too-many")])
        .await;
    assert_eq!(
        over.status, 400,
        "case \"application quota\": expected HTTP 400, got {} and body {}",
        over.status, over.body
    );
    assert_eq!(
        over.error_code(),
        "TooManyApplicationsException",
        "case \"application quota\": unexpected error code in {}",
        over.body
    );
    let listed = emulator.call("DescribeApplications", &[]).await;
    assert_eq!(
        listed
            .values_at("Applications/member/ApplicationName")
            .len(),
        eb::state::MAX_APPLICATIONS,
        "case \"application quota\": the refused create must not have been stored, got {}",
        listed.body
    );

    // Environments: the same, on a store whose application quota is already full.
    let emulator = Emulator::new();
    assert_eq!(
        emulator
            .call("CreateApplication", &[("ApplicationName", "quota")])
            .await
            .status,
        200,
        "expected 200 from CreateApplication"
    );
    for index in 0..eb::state::MAX_ENVIRONMENTS {
        let name = format!("env-{index:04}");
        let created = emulator
            .call(
                "CreateEnvironment",
                &[
                    ("ApplicationName", "quota"),
                    ("EnvironmentName", name.as_str()),
                    ("SolutionStackName", SOLUTION_STACK.as_str()),
                ],
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: expected 200 while filling the environment quota, got {} and body {}",
            created.status, created.body
        );
    }
    let over = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", "quota"),
                ("EnvironmentName", "env-one-too-many"),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
            ],
        )
        .await;
    assert_eq!(
        over.status, 400,
        "case \"environment quota\": expected HTTP 400, got {} and body {}",
        over.status, over.body
    );
    assert_eq!(
        over.error_code(),
        "TooManyEnvironmentsException",
        "case \"environment quota\": unexpected error code in {}",
        over.body
    );
}

/// Every operation whose handler still carries the scaffold marker, as the
/// `@op <Name>` its header declares, read from this crate's own sources.
///
/// The marker line `//! SCAFFOLDED, NOT IMPLEMENTED:` is the stub census, so
/// the census — not a name written here — is what the probe below walks: an
/// operation implemented by a later change leaves the list by itself instead
/// of failing a test that named it.
fn scaffolded_operations() -> Vec<String> {
    let ops = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ops");
    let mut operations = Vec::new();
    for entry in std::fs::read_dir(&ops).expect("this crate has a src/ops directory") {
        let path = entry.expect("a readable directory entry").path();
        if path.extension().is_none_or(|extension| extension != "rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("a readable handler");
        if !source
            .lines()
            .any(|line| line.starts_with("//! SCAFFOLDED, NOT IMPLEMENTED:"))
        {
            continue;
        }
        let operation = source
            .lines()
            .find_map(|line| line.strip_prefix("//! @op "))
            .unwrap_or_else(|| panic!("{} declares no @op name", path.display()))
            .trim()
            .to_string();
        operations.push(operation);
    }
    operations.sort();
    operations
}

/// RULE: no handler of this service carries the scaffold marker any more — every
/// operation the model binds is implemented, so the stub census
/// ([`scaffolded_operations`]) is empty.
///
/// A stub answers 501 `NotImplemented`, which is correct for a stub and never an
/// empty 200; the census, not a name written here, is what is asserted, so an
/// operation that regressed to a scaffold fails this test by reappearing in it.
#[test]
fn no_operation_of_this_service_is_scaffolded() {
    let operations = scaffolded_operations();
    assert!(
        operations.is_empty(),
        "case \"the stub census\": expected every operation to be implemented, got {operations:?}"
    );
}
