// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon DocumentDB cluster lifecycle.
//!
//! Every test here drives the service over its real wire protocol (awsQuery form bodies
//! in, XML out) through the same [`MultiServiceRouter`] the production router uses, so a
//! response that is not computed from persisted state cannot pass: a create is always
//! read back through a describe, and a delete is always followed by the describe that
//! must no longer find the resource.
//!
//! The documented rules the service enforces are asserted over TABLES spanning the class
//! the rule generalises over, with each published bound and one step past it. Every rule's
//! source is the Smithy model member documentation, cited on the constant it feeds in
//! `src/state.rs`.
//!
//! The wire harness below ([`Docdb`], [`texts`] and the readers around them) is shared with
//! the other suites in this target — see `tests/all/subnet_and_parameter_groups.rs` — so
//! every suite drives the service exactly the same way.

use std::sync::Arc;

use service_com_amazonaws_docdb_amazonrdsv19::state;
use service_com_amazonaws_docdb_amazonrdsv19::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for; the ARNs and endpoint host names the service
/// reports must sit in it.
pub(crate) const REGION: &str = "us-east-1";

/// The awsQuery API version an SDK sends with every DocumentDB request.
pub(crate) const API_VERSION: &str = "2014-10-31";

/// One emulated DocumentDB behind a router, driven over the wire.
///
/// `pub(crate)` so the other suites in this test crate drive the service exactly as this
/// one does, over one harness rather than a copy per suite.
///
/// This router carries ONLY the DocumentDB service, so a request reaches it by naming
/// DocumentDB itself ([`ROUTE_KEY`]) — the shared `rds` signing name resolves to Amazon RDS
/// (see `src/service.rs`). The shared front door's own path — an `rds`-signed request whose
/// resource this model serves — cannot be mounted from this crate (the RDS crate depends on
/// this one), so it is pinned in
/// `core/router/tests/all/rds_shared_query_front_door_routing.rs` instead.
pub(crate) struct Docdb {
    router: MultiServiceRouter,
    /// The ONE shared engine the service was built on, so a suite can reach the stores that
    /// live in it beside this service's own — the register of the data planes this emulator
    /// serves, which is where an in-process client of a published endpoint opens its
    /// connection (`tests/all/instance_data_plane.rs`).
    engine: Arc<engine::InMemoryEngine>,
}

/// The route key a request that addresses Amazon DocumentDB by its own name resolves to —
/// its sdkId, lowercased — and the one key this service claims; see `src/service.rs`.
pub(crate) const ROUTE_KEY: &str = "docdb";

/// What the service answered: the HTTP status, the `content-type` it was sent under, and
/// the XML body.
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) content_type: String,
    pub(crate) xml: String,
}

impl Docdb {
    /// A router carrying only this service, over one shared engine — exactly what
    /// `core/router` builds, minus the other services.
    pub(crate) fn new() -> Self {
        Self::build(None)
    }

    /// A harness whose emulator is SERVED at `base` — the data port's published URL, which
    /// `OperationContext::endpoint` reports and which an address a resource announces is
    /// read off where one is.
    pub(crate) fn served_at(base: &str) -> Self {
        Self::build(Some(
            smithy_server_runtime::EndpointUrl::parse(base)
                .expect("the test's served base is an absolute https URL with a host"),
        ))
    }

    fn build(base: Option<smithy_server_runtime::EndpointUrl>) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not SigV4 verification.
        router.set_request_authentication(false);
        if let Some(base) = base {
            router.set_url_base(base);
        }
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// The shared engine this harness built the service on; see the field's own note.
    pub(crate) fn engine(&self) -> &Arc<engine::InMemoryEngine> {
        &self.engine
    }

    /// This service AND Amazon EC2 on ONE engine, with an EC2 client over the router's own
    /// in-process callback — the very seam a `SubnetIds` member is resolved through.
    ///
    /// A subnet is EC2's resource, so what a subnet group reports about the subnets it holds
    /// can only be driven end to end with the owner of subnets mounted beside this service,
    /// exactly as the assembled emulator mounts both. Used by
    /// `tests/all/subnet_and_parameter_groups.rs`.
    pub(crate) fn with_ec2() -> (Self, ::aws_sdk_ec2::Client) {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not the subject of these tests; the requests still carry the SigV4
        // credential scope, which is what routes them and names the region.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
            Arc::clone(&engine),
        ));
        let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
        let config = ::aws_sdk_ec2::Config::builder()
            .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
            .region(::aws_sdk_ec2::config::Region::new(REGION.to_string()))
            .endpoint_url(format!("https://ec2.{REGION}.amazonaws.com"))
            .credentials_provider(callback.aws_credentials_provider())
            .http_client(callback.http_client())
            // The transport is in-process: nothing to retry, time out, cache an identity for
            // or watch for a stalled stream.
            .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
            .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                ::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        (
            Self { router, engine },
            ::aws_sdk_ec2::Client::from_conf(config),
        )
    }

    /// Call `action` with the given query parameters and return the raw reply.
    pub(crate) async fn call(&self, action: &str, params: &[(&str, &str)]) -> Reply {
        self.dispatch(None, action, params).await
    }

    /// [`Self::call`] with the request ADDRESSED to `host` — the `Host` header a client
    /// configured with one endpoint URL sends. What a resource announces is read against
    /// it, so a suite asserting on an announced host drives the service through this.
    pub(crate) async fn call_addressed_to(
        &self,
        host: &str,
        action: &str,
        params: &[(&str, &str)],
    ) -> Reply {
        self.dispatch(Some(host), action, params).await
    }

    async fn dispatch(
        &self,
        host: Option<&str>,
        action: &str,
        params: &[(&str, &str)],
    ) -> Reply {
        let mut body = format!("Action={action}&Version={API_VERSION}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let mut builder = http::Request::builder()
            .method(http::Method::POST)
            .uri(format!("https://{ROUTE_KEY}.{REGION}.amazonaws.com/"))
            .header(
                http::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            // The routing key every front door reads: the SigV4 credential scope, which
            // names the service and the region. DocumentDB SHARES the `rds` signing name
            // with Amazon RDS and Amazon Neptune, and that shared name resolves to Amazon
            // RDS, so a request for THIS service names it: `docdb` (see `ROUTE_KEY` in
            // `src/service.rs`).
            .header(
                http::header::AUTHORIZATION,
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDDOCDBTESTKEY/20260101/{REGION}/{ROUTE_KEY}/\
                     aws4_request, SignedHeaders=host, Signature=00"
                ),
            );
        if let Some(host) = host {
            builder = builder.header(http::header::HOST, host);
        }
        let request = builder
            .body(bytes::Bytes::from(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        Reply {
            status: response.status().as_u16(),
            content_type: response
                .headers()
                .get(http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string(),
            xml: String::from_utf8_lossy(response.body()).into_owned(),
        }
    }

    /// Call `action` and require a 200.
    pub(crate) async fn ok(&self, case: &str, action: &str, params: &[(&str, &str)]) -> String {
        let reply = self.call(action, params).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected {action} to answer 200, got {} with {}",
            reply.status, reply.xml
        );
        reply.xml
    }
}

/// Percent-encode one query parameter value for the form body.
///
/// The encoding is the maintained `percent-encoding` crate's, a dev-dependency of this
/// crate: `NON_ALPHANUMERIC` escapes every byte outside `[A-Za-z0-9]`, which is a superset
/// of what an `application/x-www-form-urlencoded` component must escape — and an
/// over-escaped component decodes to exactly the same bytes, so the framework's own form
/// decoder reads back what the test sent.
///
/// `pub(crate)` so the sibling suites of this one test target encode their form bodies the
/// same way rather than each deriving an encoder.
pub(crate) fn form_encode(value: &str) -> String {
    percent_encoding::percent_encode(value.as_bytes(), percent_encoding::NON_ALPHANUMERIC)
        .to_string()
}

/// Wait out the window in which the clock still owes a new resource its `available`
/// status. The emulator never sleeps for it; a caller that wants to see the settled
/// status has to let the clock pass, as it would against the real service.
///
/// `std::thread::sleep` rather than `tokio::time::sleep` because the workspace's tokio
/// does not enable the `time` feature. Each test runs on its own thread, so blocking it is
/// harmless.
pub(crate) fn let_the_clock_catch_up() {
    std::thread::sleep(state::PROVISIONING_DELAY * 2);
}

/// Unescape the five XML entities the response serializer emits.
pub(crate) fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Every element text in document order whose name is `tag`.
///
/// A minimal reader, not a general XML parser, and for the same reason as
/// [`form_encode`]: an XML crate would be the right tool but cannot be added without
/// touching `Cargo.lock`. It is sound for exactly these responses — the awsQuery
/// serializer emits no comments and no CDATA, and never nests an element inside another
/// of the same name — and it matches the element's FULL name (the character after it must
/// end the start tag or separate an attribute), so `<Status>` never matches
/// `<StatusInfos>` while `<DBCluster xmlns="...">` still does match `DBCluster`.
///
/// `pub(crate)` so the sibling suites of this one test target read a response with the same
/// reader rather than each deriving one.
pub(crate) fn texts(xml: &str, tag: &str) -> Vec<String> {
    let needle = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(at) = rest.find(needle.as_str()) {
        let after_name = &rest[at + needle.len()..];
        let ends_the_name = after_name
            .chars()
            .next()
            .is_some_and(|next| next == '>' || next == '/' || next.is_whitespace());
        if !ends_the_name {
            rest = after_name;
            continue;
        }
        let Some(gt) = after_name.find('>') else {
            panic!("unterminated <{tag} in {xml}");
        };
        if after_name[..gt].ends_with('/') {
            // An empty element: `<Foo/>` carries no text.
            out.push(String::new());
            rest = &after_name[gt + 1..];
            continue;
        }
        let body = &after_name[gt + 1..];
        let Some(end) = body.find(close.as_str()) else {
            panic!("unclosed <{tag}> in {xml}");
        };
        out.push(unescape(&body[..end]));
        rest = &body[end + close.len()..];
    }
    out
}

/// The first `tag`'s text, or `None` when the response carries no such element.
pub(crate) fn opt_text(xml: &str, tag: &str) -> Option<String> {
    texts(xml, tag).into_iter().next()
}

/// The first `tag`'s text; panics when absent, quoting the response.
pub(crate) fn text(xml: &str, tag: &str) -> String {
    opt_text(xml, tag).unwrap_or_else(|| panic!("no <{tag}> in {xml}"))
}

/// How many `tag` elements the response carries.
pub(crate) fn count(xml: &str, tag: &str) -> usize {
    texts(xml, tag).len()
}

/// The awsQuery error envelope's `<Code>`.
pub(crate) fn error_code(xml: &str) -> String {
    text(xml, "Code")
}

// ---------------------------------------------------------------------------
// The lifecycle the issue describes, end to end
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_cluster_lifecycle_is_computed_from_persisted_state() {
    let docdb = Docdb::new();

    // 1. CreateDBCluster inserts a record and answers with it.
    let created = docdb
        .ok(
            "create",
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "my-cluster"),
                ("Engine", "docdb"),
                ("MasterUsername", "admin"),
                ("MasterUserPassword", "secret99"),
            ],
        )
        .await;
    let resource_id = text(&created, "DbClusterResourceId");
    assert_eq!(
        text(&created, "DBClusterIdentifier"),
        "my-cluster",
        "case \"create\": expected the stored identifier, got {created}"
    );
    assert_eq!(
        text(&created, "Status"),
        "creating",
        "case \"create\": a cluster AWS returns provisioning must not be reported ready, \
         got {created}"
    );
    assert_eq!(
        text(&created, "Port"),
        "27017",
        "case \"create\": expected DocumentDB's default port, got {created}"
    );
    assert_eq!(
        text(&created, "DBClusterArn"),
        format!("arn:aws:rds:{REGION}:000000000000:cluster:my-cluster"),
        "case \"create\": expected the regional cluster ARN, got {created}"
    );
    assert!(
        resource_id.starts_with("cluster-"),
        "case \"create\": expected a cluster-prefixed resource id, got {resource_id:?}"
    );
    // The cluster's endpoints are addresses this emulator answers its document wire on, which
    // the create opened through the core: an endpoint is an address the service ANSWERS on, and
    // a name in Amazon DocumentDB's own DNS zone resolves to nothing here. The suite that
    // drives a session on the published pair is `tests/all/cluster_endpoint_own_address.rs`;
    // here the pair is only read back as the create published it.
    let endpoint = text(&created, "Endpoint");
    assert!(
        !endpoint.ends_with("docdb.amazonaws.com")
            && endpoint
                .parse::<std::net::IpAddr>()
                .is_ok_and(|host| host.is_loopback()),
        "case \"create\": the cluster endpoint must be an address this emulator answers on, \
         got {endpoint:?} in {created}"
    );
    assert_eq!(
        text(&created, "ReaderEndpoint"),
        endpoint,
        "case \"create\": the reader endpoint fronts the same plane, so it is published at the \
         address that plane is answered at, got {created}"
    );
    assert_eq!(
        count(&created, "DBClusterMember"),
        0,
        "case \"create\": a cluster with no instances has no members, got {created}"
    );

    // Re-creating the same identifier is the modeled already-exists fault.
    let duplicate = docdb
        .call(
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "MY-CLUSTER"),
                ("Engine", "docdb"),
            ],
        )
        .await;
    assert_eq!(
        duplicate.status, 400,
        "case \"duplicate cluster\": expected 400, got {} with {}",
        duplicate.status, duplicate.xml
    );
    assert_eq!(
        error_code(&duplicate.xml),
        "DBClusterAlreadyExistsFault",
        "case \"duplicate cluster\": got {}",
        duplicate.xml
    );

    // The clock owes the cluster `available`; a describe materialises it.
    let_the_clock_catch_up();
    let described = docdb
        .ok(
            "describe one",
            "DescribeDBClusters",
            &[("DBClusterIdentifier", "my-cluster")],
        )
        .await;
    assert_eq!(
        count(&described, "DBCluster"),
        1,
        "case \"describe one\": expected exactly the named cluster, got {described}"
    );
    assert_eq!(
        text(&described, "Status"),
        "available",
        "case \"describe one\": the clock owed this cluster `available`, got {described}"
    );
    assert_eq!(
        text(&described, "DbClusterResourceId"),
        resource_id,
        "case \"describe one\": the resource id must be stable across reads, got {described}"
    );

    // 2. CreateDBInstance against an unknown cluster is the modeled 404.
    let orphan = docdb
        .call(
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "orphan-1"),
                ("DBInstanceClass", "db.r5.large"),
                ("Engine", "docdb"),
                ("DBClusterIdentifier", "no-such-cluster"),
            ],
        )
        .await;
    assert_eq!(
        orphan.status, 404,
        "case \"instance without a cluster\": expected 404, got {} with {}",
        orphan.status, orphan.xml
    );
    assert_eq!(
        error_code(&orphan.xml),
        "DBClusterNotFoundFault",
        "case \"instance without a cluster\": got {}",
        orphan.xml
    );

    // 2b. The real create stores the instance and joins it to the cluster.
    let instance = docdb
        .ok(
            "create instance",
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "my-instance-1"),
                ("DBInstanceClass", "db.r5.large"),
                ("Engine", "docdb"),
                ("DBClusterIdentifier", "my-cluster"),
            ],
        )
        .await;
    assert_eq!(
        text(&instance, "DBInstanceStatus"),
        "creating",
        "case \"create instance\": a new instance is provisioning, got {instance}"
    );
    assert_eq!(
        text(&instance, "DBClusterIdentifier"),
        "my-cluster",
        "case \"create instance\": the instance must carry its cluster, got {instance}"
    );
    // The instance's endpoint is an address its own socket answers at, which the create
    // opened through the core: an address of this emulator's, on the engine port its cluster
    // publishes. The data-plane suite (`tests/all/instance_data_plane.rs`) is what drives a
    // session on it; here the pair is only read back as the create published it.
    let address = text(&instance, "Address");
    let port = text(&instance, "Port");
    assert!(
        !address.ends_with("docdb.amazonaws.com"),
        "case \"create instance\": the endpoint must be an address this emulator answers on, \
         not a name in Amazon DocumentDB's own DNS zone, got {address:?} in {instance}"
    );
    assert!(
        address.parse::<std::net::IpAddr>().is_ok(),
        "case \"create instance\": the endpoint must name the address its socket is bound \
         on, got {address:?} in {instance}"
    );
    assert!(
        port.parse::<u16>().is_ok_and(|port| port > 0),
        "case \"create instance\": the endpoint port is the one the core assigned its \
         socket, got {port:?} in {instance}"
    );

    let duplicate_instance = docdb
        .call(
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "my-instance-1"),
                ("DBInstanceClass", "db.r5.xlarge"),
                ("Engine", "docdb"),
                ("DBClusterIdentifier", "my-cluster"),
            ],
        )
        .await;
    assert_eq!(
        duplicate_instance.status, 400,
        "case \"duplicate instance\": expected 400, got {} with {}",
        duplicate_instance.status, duplicate_instance.xml
    );
    assert_eq!(
        error_code(&duplicate_instance.xml),
        "DBInstanceAlreadyExists",
        "case \"duplicate instance\": got {}",
        duplicate_instance.xml
    );

    // 3. The cluster now reports the instance as its writer.
    let_the_clock_catch_up();
    let with_member = docdb
        .ok(
            "describe with member",
            "DescribeDBClusters",
            &[("DBClusterIdentifier", "my-cluster")],
        )
        .await;
    assert_eq!(
        count(&with_member, "DBClusterMember"),
        1,
        "case \"describe with member\": expected one member, got {with_member}"
    );
    assert_eq!(
        text(&with_member, "IsClusterWriter"),
        "true",
        "case \"describe with member\": a cluster's first member is its writer, \
         got {with_member}"
    );
    assert_eq!(
        text(&with_member, "DBClusterParameterGroupStatus"),
        "in-sync",
        "case \"describe with member\": got {with_member}"
    );

    // 4. DescribeDBInstances reports the instance from state.
    let instances = docdb.ok("describe instances", "DescribeDBInstances", &[]).await;
    assert_eq!(
        count(&instances, "DBInstance"),
        1,
        "case \"describe instances\": expected one instance, got {instances}"
    );
    assert_eq!(
        text(&instances, "DBInstanceClass"),
        "db.r5.large",
        "case \"describe instances\": the class must come from state, got {instances}"
    );
    assert_eq!(
        text(&instances, "DBInstanceStatus"),
        "available",
        "case \"describe instances\": the clock owed this instance `available`, \
         got {instances}"
    );
    assert!(
        !text(&instances, "InstanceCreateTime").is_empty(),
        "case \"describe instances\": expected an InstanceCreateTime, got {instances}"
    );

    // 5. ModifyDBCluster mutates the record, and the describe shows it.
    let modified = docdb
        .ok(
            "modify",
            "ModifyDBCluster",
            &[
                ("DBClusterIdentifier", "my-cluster"),
                ("BackupRetentionPeriod", "7"),
                ("DeletionProtection", "true"),
            ],
        )
        .await;
    assert_eq!(
        text(&modified, "BackupRetentionPeriod"),
        "7",
        "case \"modify\": got {modified}"
    );
    let after_modify = docdb
        .ok(
            "describe after modify",
            "DescribeDBClusters",
            &[("DBClusterIdentifier", "my-cluster")],
        )
        .await;
    assert_eq!(
        text(&after_modify, "BackupRetentionPeriod"),
        "7",
        "case \"describe after modify\": the mutation must be persisted, got {after_modify}"
    );
    assert_eq!(
        text(&after_modify, "DeletionProtection"),
        "true",
        "case \"describe after modify\": got {after_modify}"
    );
    // A member reads its backup retention off its cluster, so the change shows there too.
    let member_after_modify = docdb
        .ok("instances after modify", "DescribeDBInstances", &[])
        .await;
    assert_eq!(
        text(&member_after_modify, "BackupRetentionPeriod"),
        "7",
        "case \"instances after modify\": a member inherits its cluster's retention, \
         got {member_after_modify}"
    );

    // 6. Deletion is refused while deletion protection is on.
    let protected = docdb
        .call(
            "DeleteDBCluster",
            &[
                ("DBClusterIdentifier", "my-cluster"),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await;
    assert_eq!(
        protected.status, 400,
        "case \"delete while protected\": expected 400, got {} with {}",
        protected.status, protected.xml
    );
    assert_eq!(
        error_code(&protected.xml),
        "InvalidDBClusterStateFault",
        "case \"delete while protected\": got {}",
        protected.xml
    );

    // ... and still refused, once protection is off, while a member instance remains.
    docdb
        .ok(
            "drop protection",
            "ModifyDBCluster",
            &[
                ("DBClusterIdentifier", "my-cluster"),
                ("DeletionProtection", "false"),
            ],
        )
        .await;
    let with_members = docdb
        .call(
            "DeleteDBCluster",
            &[
                ("DBClusterIdentifier", "my-cluster"),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await;
    assert_eq!(
        with_members.status, 400,
        "case \"delete with members\": expected 400, got {} with {}",
        with_members.status, with_members.xml
    );
    assert_eq!(
        error_code(&with_members.xml),
        "InvalidDBClusterStateFault",
        "case \"delete with members\": got {}",
        with_members.xml
    );

    // 7. Deleting the instance removes it from state and from the cluster's members.
    let deleted_instance = docdb
        .ok(
            "delete instance",
            "DeleteDBInstance",
            &[("DBInstanceIdentifier", "my-instance-1")],
        )
        .await;
    assert_eq!(
        text(&deleted_instance, "DBInstanceStatus"),
        "deleting",
        "case \"delete instance\": got {deleted_instance}"
    );
    let gone_instance = docdb
        .call(
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", "my-instance-1")],
        )
        .await;
    assert_eq!(
        gone_instance.status, 404,
        "case \"describe deleted instance\": expected 404, got {} with {}",
        gone_instance.status, gone_instance.xml
    );
    assert_eq!(
        error_code(&gone_instance.xml),
        "DBInstanceNotFound",
        "case \"describe deleted instance\": got {}",
        gone_instance.xml
    );
    let emptied = docdb
        .ok(
            "describe after member delete",
            "DescribeDBClusters",
            &[("DBClusterIdentifier", "my-cluster")],
        )
        .await;
    assert_eq!(
        count(&emptied, "DBClusterMember"),
        0,
        "case \"describe after member delete\": the member must be gone from the cluster, \
         got {emptied}"
    );

    // 8. Now the cluster deletes, and is unobservable afterwards.
    let deleted = docdb
        .ok(
            "delete cluster",
            "DeleteDBCluster",
            &[
                ("DBClusterIdentifier", "my-cluster"),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await;
    assert_eq!(
        text(&deleted, "Status"),
        "deleting",
        "case \"delete cluster\": got {deleted}"
    );
    let gone = docdb
        .call("DescribeDBClusters", &[("DBClusterIdentifier", "my-cluster")])
        .await;
    assert_eq!(
        gone.status, 404,
        "case \"describe deleted cluster\": expected 404, got {} with {}",
        gone.status, gone.xml
    );
    assert_eq!(
        error_code(&gone.xml),
        "DBClusterNotFoundFault",
        "case \"describe deleted cluster\": got {}",
        gone.xml
    );
    let listed = docdb.ok("list after delete", "DescribeDBClusters", &[]).await;
    assert_eq!(
        count(&listed, "DBCluster"),
        0,
        "case \"list after delete\": nothing must remain, got {listed}"
    );
}

// ---------------------------------------------------------------------------
// Documented defaults
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_cluster_created_without_optional_members_reports_the_documented_defaults() {
    let docdb = Docdb::new();
    let created = docdb
        .ok(
            "bare create",
            "CreateDBCluster",
            &[("DBClusterIdentifier", "bare"), ("Engine", "docdb")],
        )
        .await;

    // Each expectation is the member doc's own "Default:" line; see src/state.rs.
    for (member, expected) in [
        ("BackupRetentionPeriod", "1"),
        ("Port", "27017"),
        ("DeletionProtection", "false"),
        ("StorageEncrypted", "false"),
        ("CopyTagsToSnapshot", "false"),
        ("MultiAZ", "false"),
        ("EngineVersion", state::DEFAULT_ENGINE_VERSION),
    ] {
        assert_eq!(
            text(&created, member),
            expected,
            "case {member:?}: expected the documented default {expected:?}, got {created}"
        );
    }
    // `StorageType` "isn't returned when you set it to standard", and standard is the
    // default, so a bare create reports none at all.
    assert_eq!(
        opt_text(&created, "StorageType"),
        None,
        "case \"StorageType\": the default storage type is not reported, got {created}"
    );
    // A cluster the caller gave no zones is spread over three zones of its own region.
    assert_eq!(
        texts(&created, "AvailabilityZone"),
        vec![
            format!("{REGION}a"),
            format!("{REGION}b"),
            format!("{REGION}c")
        ],
        "case \"AvailabilityZones\": got {created}"
    );
    // The windows keep their documented shapes: `hh24:mi-hh24:mi` and
    // `ddd:hh24:mi-ddd:hh24:mi`, 30 minutes long, and they do not conflict.
    let backup_window = text(&created, "PreferredBackupWindow");
    let maintenance_window = text(&created, "PreferredMaintenanceWindow");
    assert!(
        backup_window.len() == 11 && backup_window.contains('-') && backup_window.ends_with(":30"),
        "case \"PreferredBackupWindow\": expected hh24:mi-hh24:mi, got {backup_window:?}"
    );
    assert!(
        maintenance_window.split('-').count() == 2
            && maintenance_window.split(':').count() == 5
            && maintenance_window.ends_with(":30"),
        "case \"PreferredMaintenanceWindow\": expected ddd:hh24:mi-ddd:hh24:mi, \
         got {maintenance_window:?}"
    );

    // An explicit non-default storage type IS reported.
    let iopt1 = docdb
        .ok(
            "iopt1 create",
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "iopt-cluster"),
                ("Engine", "docdb"),
                ("StorageType", "iopt1"),
            ],
        )
        .await;
    assert_eq!(
        opt_text(&iopt1, "StorageType").as_deref(),
        Some("iopt1"),
        "case \"iopt1 create\": a non-default storage type is returned, got {iopt1}"
    );
}

// ---------------------------------------------------------------------------
// Documented input rules, over tables
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_identifier_rule_holds_for_every_identifier_the_service_takes() {
    // Source: the identical "Constraints" list on
    // `CreateDBClusterMessage$DBClusterIdentifier`,
    // `CreateDBInstanceMessage$DBInstanceIdentifier` and
    // `ModifyDBClusterMessage$NewDBClusterIdentifier` — 1 to 63 letters, numbers or
    // hyphens, first character a letter, no trailing hyphen, no two consecutive hyphens.
    let sixty_three = format!("a{}", "b".repeat(62));
    let sixty_four = format!("a{}", "b".repeat(63));
    let cases: Vec<(&str, String, bool)> = vec![
        ("single letter", "a".to_string(), true),
        ("letters and digits", "cluster7".to_string(), true),
        ("one hyphen", "my-cluster".to_string(), true),
        ("mixed case is lowercased", "MyCluster".to_string(), true),
        ("at the length bound", sixty_three.clone(), true),
        ("one past the length bound", sixty_four, false),
        ("empty", String::new(), false),
        ("leading digit", "7cluster".to_string(), false),
        ("leading hyphen", "-cluster".to_string(), false),
        ("trailing hyphen", "cluster-".to_string(), false),
        ("two consecutive hyphens", "my--cluster".to_string(), false),
        ("underscore", "my_cluster".to_string(), false),
        ("dot", "my.cluster".to_string(), false),
    ];

    for (case, identifier, valid) in &cases {
        let docdb = Docdb::new();
        let reply = docdb
            .call(
                "CreateDBCluster",
                &[("DBClusterIdentifier", identifier), ("Engine", "docdb")],
            )
            .await;
        if *valid {
            assert_eq!(
                reply.status, 200,
                "case {case:?} ({identifier:?}): expected 200, got {} with {}",
                reply.status, reply.xml
            );
            // "This parameter is stored as a lowercase string."
            assert_eq!(
                text(&reply.xml, "DBClusterIdentifier"),
                identifier.to_ascii_lowercase(),
                "case {case:?}: the identifier must be stored lowercased, got {}",
                reply.xml
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {case:?} ({identifier:?}): expected a refusal, got {} with {}",
                reply.status, reply.xml
            );
            assert_eq!(
                error_code(&reply.xml),
                "ValidationException",
                "case {case:?}: got {}",
                reply.xml
            );
        }
    }

    // The same rule governs the instance identifier and the rename target.
    let docdb = Docdb::new();
    docdb
        .ok(
            "host cluster",
            "CreateDBCluster",
            &[("DBClusterIdentifier", "host"), ("Engine", "docdb")],
        )
        .await;
    for (case, identifier, valid) in &cases {
        let reply = docdb
            .call(
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", identifier),
                    ("DBInstanceClass", "db.r5.large"),
                    ("Engine", "docdb"),
                    ("DBClusterIdentifier", "host"),
                ],
            )
            .await;
        let refused = reply.status == 400 && error_code(&reply.xml) == "ValidationException";
        assert_eq!(
            !refused, *valid,
            "case {case:?} ({identifier:?}): DBInstanceIdentifier must follow the same rule, \
             got {} with {}",
            reply.status, reply.xml
        );
    }
    // A cluster can only be renamed once it is available, so let the clock settle it
    // before exercising `NewDBClusterIdentifier`.
    let_the_clock_catch_up();
    for (case, identifier, valid) in &cases {
        let reply = docdb
            .call(
                "ModifyDBCluster",
                &[
                    ("DBClusterIdentifier", "host"),
                    ("NewDBClusterIdentifier", identifier),
                ],
            )
            .await;
        let refused = reply.status == 400 && error_code(&reply.xml) == "ValidationException";
        assert_eq!(
            !refused, *valid,
            "case {case:?} ({identifier:?}): NewDBClusterIdentifier must follow the same rule, \
             got {} with {}",
            reply.status, reply.xml
        );
        // Put the name back so the next row starts from the same cluster.
        if !refused {
            let renamed = identifier.to_ascii_lowercase();
            docdb
                .ok(
                    "rename back",
                    "ModifyDBCluster",
                    &[
                        ("DBClusterIdentifier", &renamed),
                        ("NewDBClusterIdentifier", "host"),
                    ],
                )
                .await;
        }
    }
}

#[tokio::test]
async fn the_numeric_and_credential_rules_hold_at_their_published_bounds() {
    let docdb = Docdb::new();

    // `BackupRetentionPeriod`: "Must be a value from 1 to 35."
    for (case, period, valid) in [
        ("one below the minimum", "0", false),
        ("the minimum", "1", true),
        ("inside the range", "7", true),
        ("the maximum", "35", true),
        ("one above the maximum", "36", false),
        ("negative", "-1", false),
    ] {
        let identifier = format!("retention-{}", period.replace('-', "neg"));
        let reply = docdb
            .call(
                "CreateDBCluster",
                &[
                    ("DBClusterIdentifier", &identifier),
                    ("Engine", "docdb"),
                    ("BackupRetentionPeriod", period),
                ],
            )
            .await;
        if valid {
            assert_eq!(
                reply.status, 200,
                "case {case:?} ({period}): expected 200, got {} with {}",
                reply.status, reply.xml
            );
            assert_eq!(
                text(&reply.xml, "BackupRetentionPeriod"),
                period,
                "case {case:?}: got {}",
                reply.xml
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {case:?} ({period}): expected a refusal, got {} with {}",
                reply.status, reply.xml
            );
        }
    }

    // `PromotionTier`: "Valid values: 0-15".
    docdb
        .ok(
            "tier host",
            "CreateDBCluster",
            &[("DBClusterIdentifier", "tiers"), ("Engine", "docdb")],
        )
        .await;
    for (case, tier, valid) in [
        ("one below the minimum", "-1", false),
        ("the minimum", "0", true),
        ("the maximum", "15", true),
        ("one above the maximum", "16", false),
    ] {
        let identifier = format!("tier-{}", tier.replace('-', "neg"));
        let reply = docdb
            .call(
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", &identifier),
                    ("DBInstanceClass", "db.r5.large"),
                    ("Engine", "docdb"),
                    ("DBClusterIdentifier", "tiers"),
                    ("PromotionTier", tier),
                ],
            )
            .await;
        if valid {
            assert_eq!(
                reply.status, 200,
                "case {case:?} ({tier}): expected 200, got {} with {}",
                reply.status, reply.xml
            );
            assert_eq!(
                text(&reply.xml, "PromotionTier"),
                tier,
                "case {case:?}: got {}",
                reply.xml
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {case:?} ({tier}): expected a refusal, got {} with {}",
                reply.status, reply.xml
            );
        }
    }

    // `MasterUsername`: "from 1 to 63 letters or numbers", first character a letter.
    // `MasterUserPassword`: "from 8 to 100 characters", printable ASCII without / " @.
    let sixty_three = format!("a{}", "b".repeat(62));
    let hundred = "p".repeat(100);
    let hundred_and_one = "p".repeat(101);
    let credential_cases: Vec<(&str, String, String, bool)> = vec![
        (
            "shortest accepted pair",
            "a".to_string(),
            "p".repeat(8),
            true,
        ),
        (
            "username at the length bound",
            sixty_three,
            "password1".to_string(),
            true,
        ),
        (
            "password at the length bound",
            "admin".to_string(),
            hundred,
            true,
        ),
        (
            "password one past the length bound",
            "admin".to_string(),
            hundred_and_one,
            false,
        ),
        (
            "password one below the minimum",
            "admin".to_string(),
            "p".repeat(7),
            false,
        ),
        (
            "username starting with a digit",
            "1admin".to_string(),
            "password1".to_string(),
            false,
        ),
        (
            "username with a hyphen",
            "ad-min".to_string(),
            "password1".to_string(),
            false,
        ),
        (
            "password with a forward slash",
            "admin".to_string(),
            "pass/word".to_string(),
            false,
        ),
        (
            "password with a double quote",
            "admin".to_string(),
            "pass\"word".to_string(),
            false,
        ),
        (
            "password with an at symbol",
            "admin".to_string(),
            "pass@word".to_string(),
            false,
        ),
    ];
    for (index, (case, username, password, valid)) in credential_cases.iter().enumerate() {
        let identifier = format!("cred-{index:02}");
        let reply = docdb
            .call(
                "CreateDBCluster",
                &[
                    ("DBClusterIdentifier", &identifier),
                    ("Engine", "docdb"),
                    ("MasterUsername", username),
                    ("MasterUserPassword", password),
                ],
            )
            .await;
        if *valid {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected 200, got {} with {}",
                reply.status, reply.xml
            );
            assert_eq!(
                text(&reply.xml, "MasterUsername"),
                *username,
                "case {case:?}: got {}",
                reply.xml
            );
            // The password is never echoed back by any operation.
            assert!(
                !reply.xml.contains(password.as_str()),
                "case {case:?}: the master password must never appear in a response, \
                 got {}",
                reply.xml
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {case:?}: expected a refusal, got {} with {}",
                reply.status, reply.xml
            );
            assert_eq!(
                error_code(&reply.xml),
                "ValidationException",
                "case {case:?}: got {}",
                reply.xml
            );
        }
    }

    // `Engine`: "Valid values: docdb".
    for (index, (case, engine, valid)) in [
        ("the only valid engine", "docdb", true),
        ("the engine, upper cased", "DOCDB", true),
        ("another RDS engine", "aurora", false),
        ("an empty engine", "", false),
    ]
    .into_iter()
    .enumerate()
    {
        let identifier = format!("engine-{index}");
        let reply = docdb
            .call(
                "CreateDBCluster",
                &[("DBClusterIdentifier", &identifier), ("Engine", engine)],
            )
            .await;
        if valid {
            assert_eq!(
                reply.status, 200,
                "case {case:?} ({engine:?}): expected 200, got {} with {}",
                reply.status, reply.xml
            );
            assert_eq!(
                text(&reply.xml, "Engine"),
                "docdb",
                "case {case:?}: got {}",
                reply.xml
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {case:?} ({engine:?}): expected a refusal, got {} with {}",
                reply.status, reply.xml
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

#[tokio::test]
async fn max_records_holds_its_bounds_and_the_marker_round_trips() {
    let docdb = Docdb::new();
    const TOTAL: usize = 25;
    for index in 1..=TOTAL {
        let identifier = format!("pager-{index:02}");
        docdb
            .ok(
                "pager create",
                "CreateDBCluster",
                &[("DBClusterIdentifier", &identifier), ("Engine", "docdb")],
            )
            .await;
    }

    // `MaxRecords`: "Default: 100  Constraints: Minimum 20, maximum 100." Each published
    // bound is honoured and one step past it is refused.
    for (case, max_records, valid) in [
        ("one below the minimum", "19", false),
        ("the minimum", "20", true),
        ("the maximum", "100", true),
        ("one above the maximum", "101", false),
        ("zero", "0", false),
    ] {
        for action in ["DescribeDBClusters", "DescribeDBInstances"] {
            let reply = docdb.call(action, &[("MaxRecords", max_records)]).await;
            if valid {
                assert_eq!(
                    reply.status, 200,
                    "case {case:?} ({action}, {max_records}): expected 200, got {} with {}",
                    reply.status, reply.xml
                );
            } else {
                assert_eq!(
                    reply.status, 400,
                    "case {case:?} ({action}, {max_records}): expected a refusal, got {} with {}",
                    reply.status, reply.xml
                );
                assert_eq!(
                    error_code(&reply.xml),
                    "ValidationException",
                    "case {case:?} ({action}): got {}",
                    reply.xml
                );
            }
        }
    }

    // No `MaxRecords` means the documented default of 100, which holds all 25.
    let all = docdb.ok("default page", "DescribeDBClusters", &[]).await;
    assert_eq!(
        count(&all, "DBCluster"),
        TOTAL,
        "case \"default page\": the default page holds every cluster, got {all}"
    );
    assert_eq!(
        opt_text(&all, "Marker"),
        None,
        "case \"default page\": a single-page answer carries no Marker, got {all}"
    );

    // A full page reports the next page's first key as its Marker; feeding it back
    // returns the remainder and no Marker.
    let first = docdb
        .ok("first page", "DescribeDBClusters", &[("MaxRecords", "20")])
        .await;
    assert_eq!(
        count(&first, "DBCluster"),
        20,
        "case \"first page\": got {first}"
    );
    let marker = text(&first, "Marker");
    assert_eq!(
        marker, "pager-20",
        "case \"first page\": `Marker` doc — \"the response includes only records beyond \
         the marker\" — so the token is this page's LAST cluster, got {first}"
    );
    let second = docdb
        .ok(
            "second page",
            "DescribeDBClusters",
            &[("MaxRecords", "20"), ("Marker", &marker)],
        )
        .await;
    assert_eq!(
        count(&second, "DBCluster"),
        TOTAL - 20,
        "case \"second page\": got {second}"
    );
    assert_eq!(
        opt_text(&second, "Marker"),
        None,
        "case \"second page\": the last page must omit Marker, got {second}"
    );
    // The two pages together are the whole ordered set, with nothing repeated.
    let mut paged = texts(&first, "DBClusterIdentifier");
    paged.extend(texts(&second, "DBClusterIdentifier"));
    assert_eq!(
        paged,
        texts(&all, "DBClusterIdentifier"),
        "case \"paged round trip\": the pages must reassemble the unpaged list"
    );
}

// ---------------------------------------------------------------------------
// Filters, renames, and the modeled 404s
// ---------------------------------------------------------------------------

#[tokio::test]
async fn describes_resolve_arns_and_honour_their_supported_filters() {
    let docdb = Docdb::new();
    for cluster in ["alpha", "beta"] {
        docdb
            .ok(
                "filter create",
                "CreateDBCluster",
                &[("DBClusterIdentifier", cluster), ("Engine", "docdb")],
            )
            .await;
        for replica in 1..=2 {
            let identifier = format!("{cluster}-{replica}");
            docdb
                .ok(
                    "filter instance",
                    "CreateDBInstance",
                    &[
                        ("DBInstanceIdentifier", &identifier),
                        ("DBInstanceClass", "db.r5.large"),
                        ("Engine", "docdb"),
                        ("DBClusterIdentifier", cluster),
                    ],
                )
                .await;
        }
    }

    // `DBClusterIdentifier` and the `db-cluster-id` filter both "accept cluster
    // identifiers and cluster Amazon Resource Names (ARNs)".
    let alpha_arn = format!("arn:aws:rds:{REGION}:000000000000:cluster:alpha");
    for (case, value) in [("by identifier", "alpha"), ("by ARN", alpha_arn.as_str())] {
        let reply = docdb
            .ok(case, "DescribeDBClusters", &[("DBClusterIdentifier", value)])
            .await;
        assert_eq!(
            texts(&reply, "DBClusterIdentifier"),
            vec!["alpha".to_string()],
            "case {case:?}: got {reply}"
        );
    }

    let filtered = docdb
        .ok(
            "db-cluster-id filter",
            "DescribeDBClusters",
            &[
                ("Filters.Filter.1.Name", "db-cluster-id"),
                ("Filters.Filter.1.Values.Value.1", &alpha_arn),
            ],
        )
        .await;
    assert_eq!(
        texts(&filtered, "DBClusterIdentifier"),
        vec!["alpha".to_string()],
        "case \"db-cluster-id filter\": got {filtered}"
    );

    // On DescribeDBInstances `db-cluster-id` selects a cluster's members, and
    // `db-instance-id` selects instances by their own id.
    let members = docdb
        .ok(
            "members of beta",
            "DescribeDBInstances",
            &[
                ("Filters.Filter.1.Name", "db-cluster-id"),
                ("Filters.Filter.1.Values.Value.1", "beta"),
            ],
        )
        .await;
    assert_eq!(
        texts(&members, "DBInstanceIdentifier"),
        vec!["beta-1".to_string(), "beta-2".to_string()],
        "case \"members of beta\": got {members}"
    );
    let one_instance = docdb
        .ok(
            "db-instance-id filter",
            "DescribeDBInstances",
            &[
                ("Filters.Filter.1.Name", "db-instance-id"),
                ("Filters.Filter.1.Values.Value.1", "alpha-2"),
            ],
        )
        .await;
    assert_eq!(
        texts(&one_instance, "DBInstanceIdentifier"),
        vec!["alpha-2".to_string()],
        "case \"db-instance-id filter\": got {one_instance}"
    );

    // "Filter names are case sensitive", so an unsupported name is refused rather than
    // silently ignored.
    for (case, name) in [
        ("wrong case", "DB-Cluster-Id"),
        ("unsupported name", "engine"),
    ] {
        let reply = docdb
            .call(
                "DescribeDBClusters",
                &[
                    ("Filters.Filter.1.Name", name),
                    ("Filters.Filter.1.Values.Value.1", "alpha"),
                ],
            )
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?} ({name:?}): expected a refusal, got {} with {}",
            reply.status, reply.xml
        );
    }

    // Unknown identifiers are the modeled 404s, for both resources.
    let unknown_cluster = docdb
        .call("DescribeDBClusters", &[("DBClusterIdentifier", "nope")])
        .await;
    assert_eq!(
        (unknown_cluster.status, error_code(&unknown_cluster.xml)),
        (404, "DBClusterNotFoundFault".to_string()),
        "case \"unknown cluster\": got {}",
        unknown_cluster.xml
    );
    let unknown_instance = docdb
        .call("DescribeDBInstances", &[("DBInstanceIdentifier", "nope")])
        .await;
    assert_eq!(
        (unknown_instance.status, error_code(&unknown_instance.xml)),
        (404, "DBInstanceNotFound".to_string()),
        "case \"unknown instance\": got {}",
        unknown_instance.xml
    );
    for action in ["ModifyDBCluster", "DeleteDBCluster"] {
        let reply = docdb
            .call(
                action,
                &[
                    ("DBClusterIdentifier", "nope"),
                    ("SkipFinalSnapshot", "true"),
                ],
            )
            .await;
        assert_eq!(
            (reply.status, error_code(&reply.xml)),
            (404, "DBClusterNotFoundFault".to_string()),
            "case \"unknown cluster on {action}\": got {}",
            reply.xml
        );
    }
    let unknown_delete = docdb
        .call("DeleteDBInstance", &[("DBInstanceIdentifier", "nope")])
        .await;
    assert_eq!(
        (unknown_delete.status, error_code(&unknown_delete.xml)),
        (404, "DBInstanceNotFound".to_string()),
        "case \"unknown instance on delete\": got {}",
        unknown_delete.xml
    );
}

#[tokio::test]
async fn renaming_a_cluster_rekeys_it_and_moves_its_arn_and_endpoints() {
    let docdb = Docdb::new();
    let created = docdb
        .ok(
            "rename create",
            "CreateDBCluster",
            &[("DBClusterIdentifier", "before"), ("Engine", "docdb")],
        )
        .await;
    let resource_id = text(&created, "DbClusterResourceId");
    // The address this cluster's own socket answers on, which it publishes for both of its
    // endpoints: the rename below must not move it, because it is the socket's and not the
    // name's.
    let endpoint_before = text(&created, "Endpoint");
    docdb
        .ok(
            "rename member",
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "before-1"),
                ("DBInstanceClass", "db.r5.large"),
                ("Engine", "docdb"),
                ("DBClusterIdentifier", "before"),
            ],
        )
        .await;
    docdb
        .ok(
            "occupied name",
            "CreateDBCluster",
            &[("DBClusterIdentifier", "taken"), ("Engine", "docdb")],
        )
        .await;
    let_the_clock_catch_up();

    // Renaming onto a name already in use is the already-exists fault.
    let clash = docdb
        .call(
            "ModifyDBCluster",
            &[
                ("DBClusterIdentifier", "before"),
                ("NewDBClusterIdentifier", "taken"),
            ],
        )
        .await;
    assert_eq!(
        (clash.status, error_code(&clash.xml)),
        (400, "DBClusterAlreadyExistsFault".to_string()),
        "case \"rename onto a taken name\": got {}",
        clash.xml
    );

    let renamed = docdb
        .ok(
            "rename",
            "ModifyDBCluster",
            &[
                ("DBClusterIdentifier", "before"),
                ("NewDBClusterIdentifier", "After"),
            ],
        )
        .await;
    assert_eq!(
        text(&renamed, "DBClusterIdentifier"),
        "after",
        "case \"rename\": the new identifier is stored lowercased, got {renamed}"
    );
    assert_eq!(
        text(&renamed, "DBClusterArn"),
        format!("arn:aws:rds:{REGION}:000000000000:cluster:after"),
        "case \"rename\": the ARN follows the name, got {renamed}"
    );
    assert_eq!(
        text(&renamed, "Endpoint"),
        endpoint_before,
        "case \"rename\": the cluster is served at the address its own socket answers on, and a \
         rename does not move a socket, got {renamed}"
    );
    assert_eq!(
        text(&renamed, "DbClusterResourceId"),
        resource_id,
        "case \"rename\": the immutable resource id must not move, got {renamed}"
    );

    // The old key is gone and the member's back-reference followed the rename.
    let old = docdb
        .call("DescribeDBClusters", &[("DBClusterIdentifier", "before")])
        .await;
    assert_eq!(
        (old.status, error_code(&old.xml)),
        (404, "DBClusterNotFoundFault".to_string()),
        "case \"old name after rename\": got {}",
        old.xml
    );
    let member = docdb
        .ok(
            "member after rename",
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", "before-1")],
        )
        .await;
    assert_eq!(
        text(&member, "DBClusterIdentifier"),
        "after",
        "case \"member after rename\": got {member}"
    );

    // `AllowMajorVersionUpgrade`: "You must allow major version upgrades when specifying
    // a value for the EngineVersion parameter that is a different major version than the
    // cluster's current version." Same major, different major without the flag, and
    // different major with it.
    //
    // Every version named here is one the engine PUBLISHES
    // (`state::engine_versions`): a version outside that catalogue is refused by its
    // own rule, which `tests/all/subnet_and_parameter_groups.rs` covers.
    let current = state::DEFAULT_ENGINE_VERSION;
    let same_major = current
        .rsplit_once('.')
        .map(|(line, _)| line.to_string())
        .expect("the default engine version carries a major.minor line");
    let other_major = state::engine_versions()
        .iter()
        .rev()
        .find(|published| {
            state::engine_major_version(published) != state::engine_major_version(current)
        })
        .copied()
        .expect("the engine publishes more than one major version");
    for (case, version, allow, expected) in [
        (
            "the same major version, named as a line",
            same_major.as_str(),
            None,
            200,
        ),
        (
            "a different major version without the flag",
            other_major,
            None,
            400,
        ),
        (
            "a different major version with the flag",
            other_major,
            Some("true"),
            200,
        ),
        (
            "back to the original major, with the flag",
            current,
            Some("true"),
            200,
        ),
    ] {
        let mut params = vec![
            ("DBClusterIdentifier", "after"),
            ("EngineVersion", version),
        ];
        if let Some(allow) = allow {
            params.push(("AllowMajorVersionUpgrade", allow));
        }
        let reply = docdb.call("ModifyDBCluster", &params).await;
        assert_eq!(
            reply.status, expected,
            "case {case:?} ({version}): expected {expected}, got {} with {}",
            reply.status, reply.xml
        );
        if expected == 200 {
            let resolved = state::resolve_engine_version(version)
                .expect("every version named here is a published one");
            assert_eq!(
                text(&reply.xml, "EngineVersion"),
                resolved,
                "case {case:?}: the new version must be persisted, got {}",
                reply.xml
            );
        } else {
            assert_eq!(
                error_code(&reply.xml),
                "ValidationException",
                "case {case:?}: got {}",
                reply.xml
            );
            let unchanged = docdb
                .ok(case, "DescribeDBClusters", &[("DBClusterIdentifier", "after")])
                .await;
            assert_ne!(
                text(&unchanged, "EngineVersion"),
                version,
                "case {case:?}: a refused modify must change nothing, got {unchanged}"
            );
        }
    }
}

#[tokio::test]
async fn members_elect_a_new_writer_when_the_writer_is_deleted() {
    let docdb = Docdb::new();
    docdb
        .ok(
            "election cluster",
            "CreateDBCluster",
            &[("DBClusterIdentifier", "election"), ("Engine", "docdb")],
        )
        .await;
    // Tiers are deliberately out of join order, so a pass that just picks the next
    // member cannot be mistaken for honouring `PromotionTier`.
    for (identifier, tier) in [("writer", "0"), ("backup-b", "9"), ("backup-a", "3")] {
        docdb
            .ok(
                "election member",
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", identifier),
                    ("DBInstanceClass", "db.r5.large"),
                    ("Engine", "docdb"),
                    ("DBClusterIdentifier", "election"),
                    ("PromotionTier", tier),
                ],
            )
            .await;
    }
    let_the_clock_catch_up();

    let before = docdb
        .ok(
            "before election",
            "DescribeDBClusters",
            &[("DBClusterIdentifier", "election")],
        )
        .await;
    assert_eq!(
        texts(&before, "IsClusterWriter"),
        vec!["true".to_string(), "false".to_string(), "false".to_string()],
        "case \"before election\": only the first member joined is the writer, got {before}"
    );
    // Members are spread over the cluster's three zones, so it reports MultiAZ.
    assert_eq!(
        text(&before, "MultiAZ"),
        "true",
        "case \"before election\": members in several zones make the cluster multi-AZ, \
         got {before}"
    );

    docdb
        .ok(
            "delete the writer",
            "DeleteDBInstance",
            &[("DBInstanceIdentifier", "writer")],
        )
        .await;
    let after = docdb
        .ok(
            "after election",
            "DescribeDBClusters",
            &[("DBClusterIdentifier", "election")],
        )
        .await;
    let members = texts(&after, "DBInstanceIdentifier");
    let writers = texts(&after, "IsClusterWriter");
    let promoted = members
        .iter()
        .zip(writers.iter())
        .find(|(_, is_writer)| is_writer.as_str() == "true")
        .map(|(identifier, _)| identifier.clone());
    assert_eq!(
        promoted.as_deref(),
        Some("backup-a"),
        "case \"after election\": the surviving member with the lowest PromotionTier is \
         promoted, got {after}"
    );
    assert_eq!(
        writers.iter().filter(|value| value.as_str() == "true").count(),
        1,
        "case \"after election\": a cluster has exactly one writer, got {after}"
    );
}

#[tokio::test]
async fn deleting_a_cluster_without_skipping_the_final_snapshot_needs_its_identifier() {
    let docdb = Docdb::new();
    docdb
        .ok(
            "snapshot cluster",
            "CreateDBCluster",
            &[("DBClusterIdentifier", "snapped"), ("Engine", "docdb")],
        )
        .await;

    // A caller acting before the clock settles the cluster sees the transient state and
    // the state error that goes with it.
    let too_early = docdb
        .call(
            "DeleteDBCluster",
            &[
                ("DBClusterIdentifier", "snapped"),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await;
    assert_eq!(
        (too_early.status, error_code(&too_early.xml)),
        (400, "InvalidDBClusterStateFault".to_string()),
        "case \"delete while creating\": got {}",
        too_early.xml
    );
    let_the_clock_catch_up();

    // `SkipFinalSnapshot`: "If SkipFinalSnapshot is false, you must specify a
    // FinalDBSnapshotIdentifier parameter. Default: false." And its mirror: skipping the
    // snapshot means none is taken, so naming one cannot be honoured.
    for (case, params) in [
        ("default, no snapshot name", vec![] as Vec<(&str, &str)>),
        (
            "explicit false, no snapshot name",
            vec![("SkipFinalSnapshot", "false")],
        ),
        (
            "skipping the snapshot yet naming one",
            vec![
                ("SkipFinalSnapshot", "true"),
                ("FinalDBSnapshotIdentifier", "snapped-final"),
            ],
        ),
    ] {
        let mut all = vec![("DBClusterIdentifier", "snapped")];
        all.extend(params);
        let reply = docdb.call("DeleteDBCluster", &all).await;
        assert_eq!(
            (reply.status, error_code(&reply.xml)),
            (400, "ValidationException".to_string()),
            "case {case:?}: got {}",
            reply.xml
        );
    }

    // With the snapshot named, the delete succeeds and the cluster is unobservable.
    let deleted = docdb
        .ok(
            "delete with a final snapshot",
            "DeleteDBCluster",
            &[
                ("DBClusterIdentifier", "snapped"),
                ("SkipFinalSnapshot", "false"),
                ("FinalDBSnapshotIdentifier", "snapped-final"),
            ],
        )
        .await;
    assert_eq!(
        text(&deleted, "Status"),
        "deleting",
        "case \"delete with a final snapshot\": got {deleted}"
    );
    let gone = docdb
        .call("DescribeDBClusters", &[("DBClusterIdentifier", "snapped")])
        .await;
    assert_eq!(
        (gone.status, error_code(&gone.xml)),
        (404, "DBClusterNotFoundFault".to_string()),
        "case \"describe after delete\": got {}",
        gone.xml
    );

    // `FinalDBSnapshotIdentifier` carries the identifier rule with ITS OWN bound: "Must
    // be from 1 to 255 letters, numbers, or hyphens" — longer than the 63 the cluster and
    // instance identifiers allow. Each published bound, and one step past it.
    for (case, length, accepted) in [
        ("at the snapshot identifier bound", state::MAX_SNAPSHOT_IDENTIFIER_LEN, true),
        (
            "one past the snapshot identifier bound",
            state::MAX_SNAPSHOT_IDENTIFIER_LEN + 1,
            false,
        ),
    ] {
        let cluster = format!("len-{length}");
        let snapshot = format!("s{}", "n".repeat(length - 1));
        docdb
            .ok(
                case,
                "CreateDBCluster",
                &[("DBClusterIdentifier", &cluster), ("Engine", "docdb")],
            )
            .await;
        let_the_clock_catch_up();
        let reply = docdb
            .call(
                "DeleteDBCluster",
                &[
                    ("DBClusterIdentifier", &cluster),
                    ("SkipFinalSnapshot", "false"),
                    ("FinalDBSnapshotIdentifier", &snapshot),
                ],
            )
            .await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case {case:?} ({length} characters): expected 200, got {} with {}",
                reply.status, reply.xml
            );
        } else {
            assert_eq!(
                (reply.status, error_code(&reply.xml)),
                (400, "ValidationException".to_string()),
                "case {case:?} ({length} characters): got {}",
                reply.xml
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Store bounds: every growth site refuses rather than growing without limit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_store_refuses_to_grow_past_the_published_record_quotas() {
    // Amazon DocumentDB's published default quota "Clusters: 40" per Region.
    let docdb = Docdb::new();
    for index in 1..=state::MAX_DB_CLUSTERS {
        let identifier = format!("quota-{index:03}");
        docdb
            .ok(
                "cluster at quota",
                "CreateDBCluster",
                &[("DBClusterIdentifier", &identifier), ("Engine", "docdb")],
            )
            .await;
    }
    let over = docdb
        .call(
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "quota-over"),
                ("Engine", "docdb"),
            ],
        )
        .await;
    assert_eq!(
        (over.status, error_code(&over.xml)),
        (403, "DBClusterQuotaExceededFault".to_string()),
        "case \"one cluster past the quota\": expected the modelled quota fault, got {}",
        over.xml
    );
    // The refused create stored nothing.
    let listed = docdb.ok("after quota", "DescribeDBClusters", &[]).await;
    assert!(
        !listed.contains("quota-over"),
        "case \"one cluster past the quota\": the refused cluster must not be stored"
    );

    // Amazon DocumentDB's published quota "Instances per cluster: 16".
    let docdb = Docdb::new();
    docdb
        .ok(
            "instance quota cluster",
            "CreateDBCluster",
            &[("DBClusterIdentifier", "members"), ("Engine", "docdb")],
        )
        .await;
    for index in 1..=state::MAX_DB_INSTANCES_PER_CLUSTER {
        let identifier = format!("member-{index:03}");
        docdb
            .ok(
                "instance at quota",
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", &identifier),
                    ("DBInstanceClass", "db.r5.large"),
                    ("Engine", "docdb"),
                    ("DBClusterIdentifier", "members"),
                ],
            )
            .await;
    }
    let over_member = docdb
        .call(
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "member-over"),
                ("DBInstanceClass", "db.r5.large"),
                ("Engine", "docdb"),
                ("DBClusterIdentifier", "members"),
            ],
        )
        .await;
    assert_eq!(
        (over_member.status, error_code(&over_member.xml)),
        (400, "InstanceQuotaExceeded".to_string()),
        "case \"one instance past the per-cluster quota\": got {}",
        over_member.xml
    );
    let members = docdb
        .ok(
            "members at quota",
            "DescribeDBClusters",
            &[("DBClusterIdentifier", "members")],
        )
        .await;
    assert_eq!(
        count(&members, "DBClusterMember"),
        state::MAX_DB_INSTANCES_PER_CLUSTER,
        "case \"one instance past the per-cluster quota\": the refused instance must not \
         have joined the cluster"
    );
}

#[tokio::test]
async fn final_snapshots_stop_at_their_quota_instead_of_growing_without_limit() {
    // The snapshot store is the one store with no reclamation path yet (every snapshot
    // operation is still scaffolded), so it is capped at Amazon DocumentDB's published
    // default quota "Manual cluster snapshots: 100" per Region. Clusters are created and
    // deleted in batches because at most MAX_DB_CLUSTERS may exist at once.
    let docdb = Docdb::new();
    const BATCH: usize = 25;
    let mut taken = 0usize;
    while taken < state::MAX_DB_CLUSTER_SNAPSHOTS {
        let batch: Vec<String> = (0..BATCH)
            .map(|offset| format!("snap-{:03}", taken + offset))
            .collect();
        for identifier in &batch {
            docdb
                .ok(
                    "snapshot batch create",
                    "CreateDBCluster",
                    &[("DBClusterIdentifier", identifier), ("Engine", "docdb")],
                )
                .await;
        }
        let_the_clock_catch_up();
        for identifier in &batch {
            let final_snapshot = format!("final-{identifier}");
            docdb
                .ok(
                    "snapshot batch delete",
                    "DeleteDBCluster",
                    &[
                        ("DBClusterIdentifier", identifier),
                        ("SkipFinalSnapshot", "false"),
                        ("FinalDBSnapshotIdentifier", &final_snapshot),
                    ],
                )
                .await;
        }
        taken += BATCH;
    }

    docdb
        .ok(
            "one snapshot too many",
            "CreateDBCluster",
            &[("DBClusterIdentifier", "snap-over"), ("Engine", "docdb")],
        )
        .await;
    let_the_clock_catch_up();
    let over = docdb
        .call(
            "DeleteDBCluster",
            &[
                ("DBClusterIdentifier", "snap-over"),
                ("SkipFinalSnapshot", "false"),
                ("FinalDBSnapshotIdentifier", "final-snap-over"),
            ],
        )
        .await;
    assert_eq!(
        (over.status, error_code(&over.xml)),
        (400, "SnapshotQuotaExceeded".to_string()),
        "case \"one snapshot past the quota\": got {}",
        over.xml
    );
    // The refusal left the cluster alone, so it is still there to delete.
    let still_there = docdb
        .ok(
            "cluster after a refused delete",
            "DescribeDBClusters",
            &[("DBClusterIdentifier", "snap-over")],
        )
        .await;
    assert_eq!(
        text(&still_there, "Status"),
        "available",
        "case \"one snapshot past the quota\": a refused delete must not delete, \
         got {still_there}"
    );

    // A final snapshot really is written to state: taking one under a name already in
    // state is the modelled already-exists fault.
    docdb
        .ok(
            "duplicate snapshot cluster",
            "CreateDBCluster",
            &[("DBClusterIdentifier", "snap-dup"), ("Engine", "docdb")],
        )
        .await;
    let_the_clock_catch_up();
    let duplicate = docdb
        .call(
            "DeleteDBCluster",
            &[
                ("DBClusterIdentifier", "snap-dup"),
                ("SkipFinalSnapshot", "false"),
                ("FinalDBSnapshotIdentifier", "final-snap-000"),
            ],
        )
        .await;
    assert_eq!(
        (duplicate.status, error_code(&duplicate.xml)),
        (400, "DBClusterSnapshotAlreadyExistsFault".to_string()),
        "case \"final snapshot name already in state\": the earlier delete must really \
         have stored its snapshot, got {}",
        duplicate.xml
    );
}

/// One row of the caller-sized-member table: the case name, the extra query parameters
/// it sends, and whether the service must accept it.
struct BoundCase {
    label: String,
    extra: Vec<(String, String)>,
    accepted: bool,
}

#[tokio::test]
async fn caller_sized_members_are_bounded_before_they_reach_the_store() {
    let docdb = Docdb::new();
    let long_string = "x".repeat(state::MAX_STRING_CHARS + 1);
    let at_bound_string = "x".repeat(state::MAX_STRING_CHARS);
    let long_tag_key = "k".repeat(state::MAX_TAG_KEY_CHARS + 1);
    let long_tag_value = "v".repeat(state::MAX_TAG_VALUE_CHARS + 1);

    // Each row is a caller-sized member at its bound (accepted) or one step past it
    // (refused): the tag count, a tag key, a tag value, a list's entry count, a list
    // entry's length, and a free-form stored string.
    let mut cases: Vec<BoundCase> = Vec::new();
    for (label, tag_count, accepted) in [
        ("tags at the count bound", state::MAX_TAGS, true),
        ("one tag past the count bound", state::MAX_TAGS + 1, false),
    ] {
        let mut params = Vec::new();
        for index in 1..=tag_count {
            params.push((format!("Tags.Tag.{index}.Key"), format!("k{index}")));
            params.push((format!("Tags.Tag.{index}.Value"), format!("v{index}")));
        }
        cases.push(BoundCase {
            label: label.to_string(),
            extra: params,
            accepted,
        });
    }
    cases.push(BoundCase {
        label: "a tag key past its bound".to_string(),
        extra: vec![
            ("Tags.Tag.1.Key".to_string(), long_tag_key),
            ("Tags.Tag.1.Value".to_string(), "v".to_string()),
        ],
        accepted: false,
    });
    cases.push(BoundCase {
        label: "a tag value past its bound".to_string(),
        extra: vec![
            ("Tags.Tag.1.Key".to_string(), "k".to_string()),
            ("Tags.Tag.1.Value".to_string(), long_tag_value),
        ],
        accepted: false,
    });
    for (label, zone_count, accepted) in [
        (
            "availability zones at the entry bound",
            state::MAX_COLLECTION_ENTRIES,
            true,
        ),
        (
            "one availability zone past the entry bound",
            state::MAX_COLLECTION_ENTRIES + 1,
            false,
        ),
    ] {
        let params = (1..=zone_count)
            .map(|index| {
                (
                    format!("AvailabilityZones.AvailabilityZone.{index}"),
                    format!("{REGION}{index}"),
                )
            })
            .collect();
        cases.push(BoundCase {
            label: label.to_string(),
            extra: params,
            accepted,
        });
    }
    cases.push(BoundCase {
        label: "a security group id past the string bound".to_string(),
        extra: vec![(
            "VpcSecurityGroupIds.VpcSecurityGroupId.1".to_string(),
            long_string.clone(),
        )],
        accepted: false,
    });
    cases.push(BoundCase {
        label: "a stored string at its bound".to_string(),
        extra: vec![("KmsKeyId".to_string(), at_bound_string)],
        accepted: true,
    });
    cases.push(BoundCase {
        label: "a stored string past its bound".to_string(),
        extra: vec![("KmsKeyId".to_string(), long_string)],
        accepted: false,
    });

    for (index, case) in cases.iter().enumerate() {
        let BoundCase {
            label,
            extra,
            accepted,
        } = case;
        let identifier = format!("bound-{index:02}");
        let mut owned = vec![
            ("DBClusterIdentifier".to_string(), identifier.clone()),
            ("Engine".to_string(), "docdb".to_string()),
        ];
        owned.extend(extra.iter().cloned());
        let params: Vec<(&str, &str)> = owned
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        let reply = docdb.call("CreateDBCluster", &params).await;
        if *accepted {
            assert_eq!(
                reply.status, 200,
                "case {label:?}: expected 200 at the bound, got {} with {}",
                reply.status, reply.xml
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {label:?}: expected a refusal past the bound, got {} with {}",
                reply.status, reply.xml
            );
            assert_eq!(
                error_code(&reply.xml),
                "ValidationException",
                "case {label:?}: got {}",
                reply.xml
            );
            let listed = docdb.ok(label, "DescribeDBClusters", &[]).await;
            assert!(
                !listed.contains(identifier.as_str()),
                "case {label:?}: a refused create must store nothing"
            );
        }
    }
}
