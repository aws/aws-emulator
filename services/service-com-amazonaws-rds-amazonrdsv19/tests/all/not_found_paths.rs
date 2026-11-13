// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the "names nothing" answers of this service's resource lookups,
//! and for the reads those lookups sit on: `DescribeDBInstances`, `DeleteDBInstance`,
//! `RebootDBInstance`, `CreateDBInstance`, `ApplyPendingMaintenanceAction`,
//! `DescribeGlobalClusters`, `DescribeEventSubscriptions` and `DescribeCertificates`.
//!
//! The rule under test, asserted over tables that span the class rather than one recorded
//! request: an operation that takes a resource NAME answers that resource's own modelled
//! not-found fault — the status and the `awsQueryError` code its own model declares — when
//! the name resolves to nothing this service holds, and answers from state when it
//! resolves. The same operation WITHOUT a name reports a (possibly empty) page instead:
//! "nothing named" and "nothing stored" are different answers.
//!
//! `ApplyPendingMaintenanceAction` is here for one part of that rule — a resource is
//! addressable only in its own region, which for that operation is a refusal of the
//! `ResourceIdentifier` it carries — while the shapes that identifier can take are covered
//! by the sibling suite `all/maintenance_action_resource_guard.rs`.
//!
//! the certificate catalog `DescribeCertificates` publishes is covered by the sibling
//! suite `all/query_catalog_reads.rs`; what this one adds for that operation is the row
//! above — an identifier outside the catalog is its modelled 404.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter` exactly
//! as the production router mounts it, and each request is a real awsQuery form POST
//! carrying the SigV4 credential scope the router keys on, so the assertions read the XML
//! the emulator answers with. The helpers below are this suite's own (the sibling
//! `db_instances.rs` keeps its own): a test module that reaches into another's private
//! harness couples the two.

use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

use service_com_amazonaws_rds_amazonrdsv19::state::{
    EventSubscriptionRecord, GlobalClusterRecord, STATUS_TRANSITION_DELAY,
};
use service_com_amazonaws_rds_amazonrdsv19::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for, unless the call names another.
const REGION: &str = "us-west-2";

/// A second region, for the rows that pin that a resource is addressable only in the
/// region it was created in.
const OTHER_REGION: &str = "eu-west-1";

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// Wait out the window in which the clock still owes a transition this service started.
/// Nothing sleeps inside the emulator; a caller that wants to see the settled status has
/// to let the clock pass, as it would against the real service.
fn wait_for_settle() {
    sleep(STATUS_TRANSITION_DELAY + Duration::from_millis(200));
}

/// A router carrying only this service, plus the engine it is mounted on — the engine is
/// handed back so a test can write a precondition no implemented operation can create yet
/// (a DB cluster, a global cluster, an event subscription).
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Verifying signatures is the emulator's concern, not this service's; these tests sign
    // nothing, so the check is off and the scope header is read for routing only.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// This service's store on `engine`, reached through the same guard a handler uses.
fn store(
    engine: &Arc<engine::InMemoryEngine>,
) -> engine::ServiceState<service_com_amazonaws_rds_amazonrdsv19::state::ComAmazonawsRdsAmazonrdsv19State>
{
    service_com_amazonaws_rds_amazonrdsv19::state::service_state(&**engine)
        .expect("this service's store is reachable on the engine the test mounted it on")
}

/// One awsQuery call against `region`, returning the HTTP status and the XML body.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2014-10-31");
    for (name, value) in params {
        assert!(
            !value.contains(['&', '=', '+', '%']),
            "test parameter {name} would need form escaping: {value:?}"
        );
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(value);
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("https://rds.amazonaws.com/")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{region}/rds/aws4_request, \
                 SignedHeaders=host, Signature=unverified"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8(response.body().to_vec()).expect("a UTF-8 response body");
    (status, body)
}

/// One awsQuery call against [`REGION`].
async fn call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    call_in(router, REGION, action, params).await
}

/// The text of every `<tag>` element in `xml`, in document order. Element names are
/// matched whole (`<Certificate>` never matches `<CertificateIdentifier>`), which is all
/// these flat awsQuery envelopes need.
fn texts(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        match after.find(&close) {
            Some(end) => {
                found.push(after[..end].to_string());
                rest = &after[end + close.len()..];
            }
            None => break,
        }
    }
    found
}

/// The text of the first `<tag>` element, or the empty string when there is none.
fn text(xml: &str, tag: &str) -> String {
    texts(xml, tag).into_iter().next().unwrap_or_default()
}

/// Assert a 200 and hand back the body.
fn expect_ok(case: &str, response: (u16, String)) -> String {
    let (status, body) = response;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

/// Assert the modelled failure: its HTTP status and its wire error `<Code>`, which is the
/// `awsQueryError` code the model declares for that fault.
fn expect_error(case: &str, response: (u16, String), expected_status: u16, expected_code: &str) {
    let (status, body) = response;
    let code = text(&body, "Code");
    assert_eq!(
        (status, code.as_str()),
        (expected_status, expected_code),
        "case {case:?}: expected HTTP {expected_status} with error code {expected_code:?}, \
         got {status} with {code:?} in {body}"
    );
    // A Sender fault: the awsQuery envelope types every client error that way, and a
    // client that branches on it must not see `Receiver` for a name that simply does not
    // resolve.
    let fault_type = text(&body, "Type");
    assert_eq!(
        fault_type, "Sender",
        "case {case:?}: expected the fault to be typed Sender, got {fault_type:?} in {body}"
    );
    // And the request id AWS carries in an error envelope, which a client correlates a
    // failure on exactly as it does a success.
    assert!(
        !text(&body, "RequestId").is_empty(),
        "case {case:?}: expected the error envelope to carry a RequestId, got {body}"
    );
}

/// Create a DB instance and wait until the clock has made it `available`.
async fn create_available_instance(router: &MultiServiceRouter, identifier: &str) {
    expect_ok(
        &format!("create {identifier}"),
        call(
            router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", identifier),
                ("Engine", "postgres"),
                ("DBInstanceClass", "db.t3.micro"),
            ],
        )
        .await,
    );
    wait_for_settle();
}

// ---------------------------------------------------------------------------
// A name that resolves to nothing: every operation answers its OWN modelled fault.
// ---------------------------------------------------------------------------

/// One row of the not-found table: the case, the action, its parameters, the HTTP status
/// the model declares for that fault and the awsQuery `<Code>` it must carry.
type NotFoundRow = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
    u16,
    &'static str,
);

/// One row of a table that varies only the call: the case, the action and its parameters.
type CallRow = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

#[tokio::test]
async fn a_name_that_resolves_to_nothing_answers_that_resources_not_found_fault() {
    let (router, _engine) = emulator();

    // The rows span the operations that take a resource name — a DB instance identifier, a
    // DB cluster identifier, a resource ARN, a global cluster identifier, a subscription
    // name and a certificate identifier — and, within each, the shapes a name can take:
    // short and long, lower and mixed case, an identifier and an ARN. Every expected
    // status and code is the one that operation's model declares
    // (`smithy_query(query="operation", operation="…")`): the `awsQueryError` code, which
    // is NOT always the shape name (`DBInstanceNotFoundFault` is sent as
    // `DBInstanceNotFound`).
    let rows: &[NotFoundRow] = &[
        (
            "DescribeDBInstances for an instance that was never created",
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", "no-such-instance")],
            404,
            "DBInstanceNotFound",
        ),
        (
            "DescribeDBInstances for a mixed-case name",
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", "No-Such-Instance-Either")],
            404,
            "DBInstanceNotFound",
        ),
        (
            "DescribeDBInstances for a 63-character name",
            "DescribeDBInstances",
            &[(
                "DBInstanceIdentifier",
                "a23456789012345678901234567890123456789012345678901234567890123",
            )],
            404,
            "DBInstanceNotFound",
        ),
        (
            "DeleteDBInstance for an instance that was never created",
            "DeleteDBInstance",
            &[
                ("DBInstanceIdentifier", "no-such-instance"),
                ("SkipFinalSnapshot", "true"),
            ],
            404,
            "DBInstanceNotFound",
        ),
        (
            "DeleteDBInstance asking for a final snapshot of an absent instance",
            "DeleteDBInstance",
            &[
                ("DBInstanceIdentifier", "no-such-instance"),
                ("FinalDBSnapshotIdentifier", "no-such-instance-final"),
            ],
            404,
            "DBInstanceNotFound",
        ),
        (
            // Neither member: for an instance that EXISTS this combination is refused
            // (`FinalDBSnapshotIdentifier` is required unless the snapshot is skipped),
            // but the named instance decides first — those members say what to do WITH an
            // instance, and there is none.
            "DeleteDBInstance for an absent instance with neither snapshot member",
            "DeleteDBInstance",
            &[("DBInstanceIdentifier", "no-such-instance")],
            404,
            "DBInstanceNotFound",
        ),
        (
            "DeleteDBInstance for an absent instance with both snapshot members",
            "DeleteDBInstance",
            &[
                ("DBInstanceIdentifier", "no-such-instance"),
                ("SkipFinalSnapshot", "true"),
                ("FinalDBSnapshotIdentifier", "no-such-instance-final"),
            ],
            404,
            "DBInstanceNotFound",
        ),
        (
            "RebootDBInstance for an instance that was never created",
            "RebootDBInstance",
            &[("DBInstanceIdentifier", "no-such-instance")],
            404,
            "DBInstanceNotFound",
        ),
        (
            // The absent instance decides the answer: there is no instance to measure the
            // Multi-AZ constraint on, so this is still the not-found fault and not a
            // refusal of `ForceFailover`.
            "RebootDBInstance with ForceFailover for an absent instance",
            "RebootDBInstance",
            &[
                ("DBInstanceIdentifier", "no-such-instance"),
                ("ForceFailover", "true"),
            ],
            404,
            "DBInstanceNotFound",
        ),
        (
            // The failure is attributed to the CLUSTER, not to the instance being created.
            "CreateDBInstance joining a cluster that does not exist",
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-member-1"),
                ("Engine", "aurora-postgresql"),
                ("DBInstanceClass", "db.r6g.large"),
                ("DBClusterIdentifier", "no-such-cluster"),
            ],
            404,
            "DBClusterNotFoundFault",
        ),
        (
            "CreateDBInstance joining a mixed-case cluster that does not exist",
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-member-2"),
                ("Engine", "aurora-mysql"),
                ("DBInstanceClass", "db.r6g.large"),
                ("DBClusterIdentifier", "No-Such-Cluster"),
            ],
            404,
            "DBClusterNotFoundFault",
        ),
        // `ApplyPendingMaintenanceAction` is NOT in this table: its `ResourceIdentifier` is
        // an ARN the caller composes, so which answer an identifier that resolves to nothing
        // earns depends on the resource KIND that ARN names. The sibling suite
        // `all/maintenance_action_resource_guard.rs` asserts that rule over the shapes such
        // an identifier can take.
        (
            "DescribeGlobalClusters for a global cluster that was never created",
            "DescribeGlobalClusters",
            &[("GlobalClusterIdentifier", "no-such-global-cluster")],
            404,
            "GlobalClusterNotFoundFault",
        ),
        (
            // `GlobalClusterIdentifier`'s pattern admits `:._` and upper case, so a name of
            // that shape must reach the same answer rather than a refusal.
            "DescribeGlobalClusters for a punctuated, mixed-case identifier",
            "DescribeGlobalClusters",
            &[("GlobalClusterIdentifier", "No-Such.Global_Cluster")],
            404,
            "GlobalClusterNotFoundFault",
        ),
        (
            "DescribeEventSubscriptions for a subscription that was never created",
            "DescribeEventSubscriptions",
            &[("SubscriptionName", "no-such-subscription")],
            404,
            "SubscriptionNotFound",
        ),
        (
            "DescribeCertificates for a certificate identifier outside the catalog",
            "DescribeCertificates",
            &[("CertificateIdentifier", "rds-ca-no-such-g9")],
            404,
            "CertificateNotFound",
        ),
        (
            "DescribeCertificates for an empty-looking identifier",
            "DescribeCertificates",
            &[("CertificateIdentifier", "rds-ca")],
            404,
            "CertificateNotFound",
        ),
    ];

    for (case, action, params, status, code) in rows {
        expect_error(case, call(&router, action, params).await, *status, code);
    }
}

#[tokio::test]
async fn the_same_operations_report_a_page_when_no_name_is_given() {
    let (router, _engine) = emulator();

    // "Nothing named" is a 404; "nothing stored" is a 200 with an empty list. Each row is
    // the unnamed form of one of the reads above, plus the element its page lives in.
    let rows: [(&str, &str, &str); 4] = [
        (
            "DescribeDBInstances over an empty store",
            "DescribeDBInstances",
            "DBInstances",
        ),
        (
            "DescribeGlobalClusters over an empty store",
            "DescribeGlobalClusters",
            "GlobalClusters",
        ),
        (
            "DescribeEventSubscriptions over an empty store",
            "DescribeEventSubscriptions",
            "EventSubscriptionsList",
        ),
        (
            // The one read whose page is a CATALOG rather than caller state: it is never
            // empty.
            "DescribeCertificates with no identifier",
            "DescribeCertificates",
            "Certificates",
        ),
    ];

    for (case, action, page_element) in rows {
        let body = expect_ok(case, call(&router, action, &[]).await);
        assert!(
            body.contains(&format!("<{page_element}")),
            "case {case:?}: expected a {page_element} page in the response, got {body}"
        );
        assert!(
            !body.contains("<Code>"),
            "case {case:?}: expected no error envelope, got {body}"
        );
    }
}

#[tokio::test]
async fn a_resource_is_addressable_only_in_the_region_it_was_created_in() {
    let (router, _engine) = emulator();
    create_available_instance(&router, "emu-regional-1").await;

    // The instance is in REGION, so the SAME name signed for another region names nothing:
    // every operation that takes it answers its own not-found fault there and serves in
    // its own region.
    let rows: [CallRow; 3] = [
        (
            "DescribeDBInstances from another region",
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", "emu-regional-1")],
        ),
        (
            "RebootDBInstance from another region",
            "RebootDBInstance",
            &[("DBInstanceIdentifier", "emu-regional-1")],
        ),
        (
            "DeleteDBInstance from another region",
            "DeleteDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-regional-1"),
                ("SkipFinalSnapshot", "true"),
            ],
        ),
    ];
    for (case, action, params) in rows {
        expect_error(
            case,
            call_in(&router, OTHER_REGION, action, params).await,
            404,
            "DBInstanceNotFound",
        );
    }

    // And the instance's own ARN resolves to no resource in the other region either: the
    // maintenance-action front door refuses an identifier that resolves to nothing on the
    // MEMBER that carried it (see `all/maintenance_action_resource_guard.rs` for the rule),
    // which is still a statement about the REGION and not about the name.
    expect_error(
        "ApplyPendingMaintenanceAction from another region",
        call_in(
            &router,
            OTHER_REGION,
            "ApplyPendingMaintenanceAction",
            &[
                (
                    "ResourceIdentifier",
                    "arn:aws:rds:us-west-2:000000000000:db:emu-regional-1",
                ),
                ("ApplyAction", "system-update"),
                ("OptInType", "immediate"),
            ],
        )
        .await,
        400,
        "InvalidParameterValue",
    );

    // In its own region the same name resolves, which is what makes the rows above a
    // statement about the region and not about the name.
    let body = expect_ok(
        "DescribeDBInstances in its own region",
        call(
            &router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", "emu-regional-1")],
        )
        .await,
    );
    assert_eq!(
        text(&body, "DBInstanceIdentifier"),
        "emu-regional-1",
        "expected the instance to be reported in its own region, got {body}"
    );
}

// ---------------------------------------------------------------------------
// A name that DOES resolve: the answer comes out of state.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rebooting_a_stored_instance_restarts_it_and_the_clock_brings_it_back() {
    let (router, _engine) = emulator();
    create_available_instance(&router, "emu-reboot-1").await;

    let body = expect_ok(
        "reboot an available instance",
        call(
            &router,
            "RebootDBInstance",
            &[("DBInstanceIdentifier", "emu-reboot-1")],
        )
        .await,
    );
    assert_eq!(
        text(&body, "DBInstanceStatus"),
        "rebooting",
        "expected the momentary outage to be reported as rebooting, got {body}"
    );

    // A caller that reads straight back still sees the outage: the status the clock owes
    // is not due yet.
    let during = expect_ok(
        "describe during the reboot",
        call(
            &router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", "emu-reboot-1")],
        )
        .await,
    );
    assert_eq!(
        text(&during, "DBInstanceStatus"),
        "rebooting",
        "expected the instance to still be rebooting, got {during}"
    );
    // And a second reboot is refused while it is in that state.
    expect_error(
        "reboot an instance that is already rebooting",
        call(
            &router,
            "RebootDBInstance",
            &[("DBInstanceIdentifier", "emu-reboot-1")],
        )
        .await,
        400,
        "InvalidDBInstanceState",
    );

    wait_for_settle();
    let after = expect_ok(
        "describe after the reboot",
        call(
            &router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", "emu-reboot-1")],
        )
        .await,
    );
    assert_eq!(
        text(&after, "DBInstanceStatus"),
        "available",
        "expected the clock to bring the instance back to available, got {after}"
    );
}

#[tokio::test]
async fn force_failover_is_refused_unless_the_instance_is_multi_az() {
    let (router, _engine) = emulator();

    // `ForceFailover`: "You can't enable force failover if the instance isn't configured
    // for Multi-AZ." Both branches of that constraint, over an instance that exists.
    for (case, identifier, multi_az, expected_status) in [
        ("single-AZ instance", "emu-failover-single", "false", 400),
        ("Multi-AZ instance", "emu-failover-multi", "true", 200),
    ] {
        expect_ok(
            case,
            call(
                &router,
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", identifier),
                    ("Engine", "postgres"),
                    ("DBInstanceClass", "db.t3.micro"),
                    ("MultiAZ", multi_az),
                ],
            )
            .await,
        );
        wait_for_settle();
        let (status, body) = call(
            &router,
            "RebootDBInstance",
            &[
                ("DBInstanceIdentifier", identifier),
                ("ForceFailover", "true"),
            ],
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for a forced failover, got \
             {status} with {body}"
        );
    }
}

#[tokio::test]
async fn applying_a_maintenance_action_reports_the_resources_own_pending_actions() {
    let (router, _engine) = emulator();
    create_available_instance(&router, "emu-maint-1").await;
    let arn = format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:emu-maint-1");

    // Every documented `ApplyAction` / `OptInType` pairing reaches the resource: the
    // resource exists, so the answer is its pending-action list, which is EMPTY because
    // nothing in this service schedules a maintenance action.
    for (action, opt_in) in [
        ("system-update", "immediate"),
        ("db-upgrade", "next-maintenance"),
        ("hardware-maintenance", "undo-opt-in"),
        ("os-upgrade", "immediate"),
        ("ca-certificate-rotation", "next-maintenance"),
    ] {
        let case = format!("{action} / {opt_in} on a stored instance");
        let body = expect_ok(
            &case,
            call(
                &router,
                "ApplyPendingMaintenanceAction",
                &[
                    ("ResourceIdentifier", &arn),
                    ("ApplyAction", action),
                    ("OptInType", opt_in),
                ],
            )
            .await,
        );
        assert_eq!(
            text(&body, "ResourceIdentifier"),
            arn,
            "case {case:?}: expected the report to name the resource, got {body}"
        );
        assert!(
            texts(&body, "PendingMaintenanceAction").is_empty(),
            "case {case:?}: this service schedules no maintenance action, so none may be \
             reported as pending, got {body}"
        );
    }

    // A value outside the published list is refused rather than applied to the resource.
    for (case, apply_action, opt_in) in [
        ("an ApplyAction AWS does not publish", "reboot-now", "immediate"),
        ("an OptInType AWS does not publish", "system-update", "later"),
    ] {
        let (status, body) = call(
            &router,
            "ApplyPendingMaintenanceAction",
            &[
                ("ResourceIdentifier", &arn),
                ("ApplyAction", apply_action),
                ("OptInType", opt_in),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the request to be refused, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn global_clusters_are_reported_out_of_state_and_filtered_by_member_region() {
    let (router, engine) = emulator();

    // The records a report must read are written straight into the store the handlers
    // read, through the same guard a handler uses, so this suite tests the REPORT rather
    // than the writer (`global_clusters.rs` covers the create). Two global clusters, whose
    // members sit in different regions, so the one documented filter (`region`) has
    // something to select on.
    {
        let state = store(&engine);
        let mut guard = state.lock();
        for (identifier, member_region) in [("emu-global-a", REGION), ("emu-global-b", OTHER_REGION)]
        {
            guard.global_clusters.insert(
                identifier.to_string(),
                GlobalClusterRecord {
                    identifier: identifier.to_string(),
                    resource_id: format!("cluster-{identifier}"),
                    status: "available".to_string(),
                    engine: Some("aurora-postgresql".to_string()),
                    engine_version: Some("15.4".to_string()),
                    engine_lifecycle_support: "open-source-rds-extended-support".to_string(),
                    database_name: None,
                    storage_encrypted: true,
                    deletion_protection: false,
                    member_cluster_arns: vec![format!(
                        "arn:aws:rds:{member_region}:{ACCOUNT}:cluster:{identifier}-primary"
                    )],
                    endpoint_host: format!("host{identifier}"),
                    pending_status: None,
                    tags: Vec::new(),
                },
            );
        }
    }

    // Named, by exact and by differing case — "this parameter isn't case-sensitive".
    for (case, requested) in [
        ("exact identifier", "emu-global-a"),
        ("mixed case identifier", "EMU-Global-A"),
    ] {
        let body = expect_ok(
            case,
            call(
                &router,
                "DescribeGlobalClusters",
                &[("GlobalClusterIdentifier", requested)],
            )
            .await,
        );
        assert_eq!(
            texts(&body, "GlobalClusterIdentifier"),
            vec!["emu-global-a".to_string()],
            "case {case:?}: expected exactly the named global cluster, got {body}"
        );
        assert_eq!(
            text(&body, "GlobalClusterArn"),
            format!("arn:aws:rds::{ACCOUNT}:global-cluster:emu-global-a"),
            "case {case:?}: a global cluster's ARN carries no region, got {body}"
        );
        assert_eq!(
            text(&body, "IsWriter"),
            "true",
            "case {case:?}: expected the primary member to be reported as the writer, got \
             {body}"
        );
    }

    // Unnamed: both, in identifier order.
    let body = expect_ok(
        "every global cluster",
        call(&router, "DescribeGlobalClusters", &[]).await,
    );
    assert_eq!(
        texts(&body, "GlobalClusterIdentifier"),
        vec!["emu-global-a".to_string(), "emu-global-b".to_string()],
        "expected both global clusters in identifier order, got {body}"
    );

    // The `region` filter: "any global cluster with at least one member (primary or
    // secondary) in the specified Amazon Web Services Regions".
    let rows: [(&str, &str, Vec<String>); 3] = [
        ("members in us-west-2", REGION, vec!["emu-global-a".to_string()]),
        (
            "members in eu-west-1",
            OTHER_REGION,
            vec!["emu-global-b".to_string()],
        ),
        ("members in a region nothing is in", "ap-south-1", Vec::new()),
    ];
    for (case, region, expected) in rows {
        let body = expect_ok(
            case,
            call(
                &router,
                "DescribeGlobalClusters",
                &[
                    ("Filters.Filter.1.Name", "region"),
                    ("Filters.Filter.1.Values.Value.1", region),
                ],
            )
            .await,
        );
        assert_eq!(
            texts(&body, "GlobalClusterIdentifier"),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }

    // A filter name this operation does not support is refused, not ignored.
    let (status, body) = call(
        &router,
        "DescribeGlobalClusters",
        &[
            ("Filters.Filter.1.Name", "engine"),
            ("Filters.Filter.1.Values.Value.1", "aurora-postgresql"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "expected an unsupported filter name to be refused, got {status} with {body}"
    );
}

#[tokio::test]
async fn event_subscriptions_are_reported_out_of_state_per_region() {
    let (router, engine) = emulator();

    // The records are written into the store directly, so this suite tests the REPORT
    // rather than the writer (`event_subscriptions.rs` covers the create) — one in each
    // region, so the report's regional scope is observable.
    {
        let state = store(&engine);
        let mut guard = state.lock();
        for (name, region) in [("emu-sub-1", REGION), ("emu-sub-2", OTHER_REGION)] {
            guard.event_subscriptions.insert(
                (region.to_string(), name.to_string()),
                EventSubscriptionRecord {
                    subscription_name: name.to_string(),
                    region: region.to_string(),
                    sns_topic_arn: format!("arn:aws:sns:{region}:{ACCOUNT}:rds-events"),
                    status: "active".to_string(),
                    subscription_creation_time: "2026-01-01 00:00:00.000".to_string(),
                    source_type: Some("db-instance".to_string()),
                    source_ids: vec![format!("{name}-source")],
                    event_categories: vec!["availability".to_string()],
                    enabled: true,
                    tags: Vec::new(),
                },
            );
        }
    }

    let body = expect_ok(
        "the named subscription",
        call(
            &router,
            "DescribeEventSubscriptions",
            &[("SubscriptionName", "emu-sub-1")],
        )
        .await,
    );
    assert_eq!(
        texts(&body, "CustSubscriptionId"),
        vec!["emu-sub-1".to_string()],
        "expected exactly the named subscription, got {body}"
    );
    assert_eq!(
        text(&body, "EventSubscriptionArn"),
        format!("arn:aws:rds:{REGION}:{ACCOUNT}:es:emu-sub-1"),
        "expected the subscription's own ARN, got {body}"
    );

    let body = expect_ok(
        "every subscription of this region",
        call(&router, "DescribeEventSubscriptions", &[]).await,
    );
    assert_eq!(
        texts(&body, "CustSubscriptionId"),
        vec!["emu-sub-1".to_string()],
        "expected only this region's subscription, got {body}"
    );

    // The other region's subscription is not addressable here, and is in its own region.
    expect_error(
        "another region's subscription",
        call(
            &router,
            "DescribeEventSubscriptions",
            &[("SubscriptionName", "emu-sub-2")],
        )
        .await,
        404,
        "SubscriptionNotFound",
    );
    let body = expect_ok(
        "the other region's subscription, in its own region",
        call_in(
            &router,
            OTHER_REGION,
            "DescribeEventSubscriptions",
            &[("SubscriptionName", "emu-sub-2")],
        )
        .await,
    );
    assert_eq!(
        texts(&body, "CustSubscriptionId"),
        vec!["emu-sub-2".to_string()],
        "expected the subscription in its own region, got {body}"
    );

    // `Filters` "isn't currently supported" by this operation: refused, not ignored.
    let (status, body) = call(
        &router,
        "DescribeEventSubscriptions",
        &[
            ("Filters.Filter.1.Name", "subscription-name"),
            ("Filters.Filter.1.Values.Value.1", "emu-sub-1"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "expected an unsupported Filters to be refused, got {status} with {body}"
    );
}
