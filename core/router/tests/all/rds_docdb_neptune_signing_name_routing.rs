// SPDX-License-Identifier: CC0-1.0

//! Amazon RDS, Amazon DocumentDB and Amazon Neptune are THREE emulated services behind
//! ONE SigV4 signing name.
//!
//! All three are generated from the same `AmazonRDSv19` model, whose service traits give
//! each of them `sigv4Name: rds` and `endpointPrefix: rds`, so the default signing-name
//! `can_handle` makes all three claim every `rds`-signed request: the router finds three
//! claimants and answers `500 AmbiguousService` — a code that exists in no AWS model —
//! for ALL of it, leaving every RDS operation unreachable. Worse for an awsQuery client,
//! that router-level refusal is a JSON `__type` body, which botocore's query parser
//! cannot read at all: the caller sees a parse failure instead of an error code.
//!
//! The rule this suite pins: a signing name shared by several emulated services resolves
//! to EXACTLY ONE of them — the service whose own identity is that name, here Amazon RDS
//! — each peer is reachable under the routing key that names IT (`docdb`, `neptune`),
//! and whatever a request gets back, it is that service's own awsQuery XML answer and
//! never a router-level refusal.
//!
//! The rows run IN ORDER against one router over one engine, so a later row can read what
//! an earlier one wrote: the last two create an option group through the shared front door
//! and read it back through it, which is what the ambiguity refusal made impossible — the
//! request never reached a handler at all.
//!
//! This suite needs the WHOLE router: the ambiguity exists only when all three services
//! are mounted, so a single-service test cannot see it. Assembling the router costs stack
//! (see `boot_serve.rs`), so it is built — and the requests driven — on an explicitly
//! sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
pub(crate) const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The awsQuery API version all three services share.
pub(crate) const API_VERSION: &str = "2014-10-31";

/// The region every signed request below is signed for.
const REGION: &str = "us-east-1";

/// How a request names the service it is for.
enum Addressed {
    /// A SigV4 credential scope naming this service — the signal the router reads first,
    /// and what every AWS SDK sends.
    Signed(&'static str),
    /// A host-style endpoint and no credentials at all: the last routing signal, which
    /// an anonymous request still carries.
    Host(&'static str),
}

/// One call: how it is addressed, the awsQuery `Action` it names, and what the ONE
/// service that owns it must answer.
struct Call {
    case: &'static str,
    addressed: Addressed,
    action: &'static str,
    /// The form parameters this call carries beyond `Action` and `Version`. Values are
    /// plain ASCII, so the body needs no escaping.
    params: &'static [(&'static str, &'static str)],
    /// The HTTP status the owning service answers, or `None` when this row only pins
    /// that the request reached exactly one service.
    expected_status: Option<u16>,
    /// The awsQuery `<Code>` the owning service answers with, when the row pins one.
    expected_code: Option<&'static str>,
    /// Elements the answer must carry, for a row that pins WHAT the owning service
    /// reported rather than only its status.
    expected_body_contains: &'static [&'static str],
    /// Elements the answer must NOT carry, for a row whose discriminator is what the
    /// owning service leaves OUT: a peer serving the same action would report them.
    expected_body_excludes: &'static [&'static str],
}

#[test]
fn rds_docdb_and_neptune_route_apart_under_one_signing_name() {
    std::thread::Builder::new()
        .name("rds-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);

            let runtime = tokio::runtime::Builder::new_current_thread()
                // IO and timers are on because the calls below reach a service that opens
                // a core listener: `CreateDBCluster` publishes the address of the engine
                // endpoint it serves the cluster's own wire on.
                .enable_all()
                .build()
                .expect("a current-thread runtime");

            // The rows differ along BOTH axes that decide where a request lands: which
            // service it names (the shared `rds` name, each peer's own key, a host-style
            // endpoint instead of a credential scope) and which operation it asks for.
            //
            // `DescribeDBClusters` is the discriminator that cannot be faked: all three
            // models declare it and each service reads its OWN cluster store, so a cluster
            // created through the `rds` key is reported there and nowhere else — the same
            // Action under three keys reports three different things, which only happens if
            // each request reached exactly one, correct service.
            // `DescribeDBEngineVersions` is another kind of discriminator: each model
            // publishes its OWN engine catalogue, so the shared `rds` key reports every
            // engine that front door serves while DocumentDB's key reports the one engine
            // its model declares and none of the others. `DescribeOptionGroups` is a third:
            // ONLY the RDS
            // model declares that action, so the shared name answers it (its modelled
            // `OptionGroupNotFoundFault` for a group nothing holds) while each peer's own key
            // answers its own `InvalidAction`.
            //
            // The last eight rows are the resource-lookup paths of the shared front door:
            // every operation that takes a resource NAME answers that resource's own
            // modelled not-found fault — the status and `awsQueryError` code ITS model
            // declares — in the awsQuery `<ErrorResponse>` envelope, carrying the
            // `<RequestId>` AWS puts in one. Each was a `500 AmbiguousService` JSON
            // refusal before, so each row also pins that one service served it.
            let calls: [Call; 21] = [
                Call {
                    // The request the issue reports as `500 AmbiguousService`.
                    case: "rds DescribeDBEngineVersions",
                    addressed: Addressed::Signed("rds"),
                    action: "DescribeDBEngineVersions",
                    params: &[],
                    expected_status: Some(200),
                    expected_code: None,
                    expected_body_contains: &[],
                    expected_body_excludes: &[],
                },
                Call {
                    case: "rds DescribeDBInstances",
                    addressed: Addressed::Signed("rds"),
                    action: "DescribeDBInstances",
                    params: &[],
                    expected_status: Some(200),
                    expected_code: None,
                    expected_body_contains: &[],
                    expected_body_excludes: &[],
                },
                Call {
                    // Amazon RDS owns the shared name, so its OWN handler serves this —
                    // not DocumentDB's or Neptune's — and the cluster it stores lands in
                    // the RDS store, which the next three rows read apart.
                    case: "rds CreateDBCluster",
                    addressed: Addressed::Signed("rds"),
                    action: "CreateDBCluster",
                    params: &[
                        ("DBClusterIdentifier", "router-cluster-rds"),
                        ("Engine", "aurora-postgresql"),
                    ],
                    expected_status: Some(200),
                    expected_code: None,
                    expected_body_contains: &[
                        "<DBClusterIdentifier>router-cluster-rds</DBClusterIdentifier>",
                        "<Engine>aurora-postgresql</Engine>",
                    ],
                    expected_body_excludes: &[],
                },
                Call {
                    case: "rds DescribeDBClusters reads the created cluster back",
                    addressed: Addressed::Signed("rds"),
                    action: "DescribeDBClusters",
                    params: &[],
                    expected_status: Some(200),
                    expected_code: None,
                    expected_body_contains: &[
                        "<DBClusterIdentifier>router-cluster-rds</DBClusterIdentifier>",
                    ],
                    expected_body_excludes: &[],
                },
                Call {
                    // DocumentDB reads its own cluster store, which that create did not
                    // touch: an EMPTY list here is what proves the request did not land on
                    // Amazon RDS.
                    case: "docdb DescribeDBClusters",
                    addressed: Addressed::Signed("docdb"),
                    action: "DescribeDBClusters",
                    params: &[],
                    expected_status: Some(200),
                    expected_code: None,
                    expected_body_contains: &["<DBClusters></DBClusters>"],
                    expected_body_excludes: &[],
                },
                Call {
                    case: "neptune DescribeDBClusters reports its own empty store",
                    addressed: Addressed::Signed("neptune"),
                    action: "DescribeDBClusters",
                    params: &[],
                    expected_status: Some(200),
                    expected_code: None,
                    expected_body_contains: &["<DBClusters></DBClusters>"],
                    expected_body_excludes: &[],
                },
                Call {
                    // DocumentDB's OWN engine-version catalogue: it publishes the one
                    // engine its model declares, so the answer naming `docdb` and no other
                    // engine is what proves this request did not land on Amazon RDS, whose
                    // answer to the very same action (the first row) carries the other
                    // engines of the shared front door as well.
                    case: "docdb DescribeDBEngineVersions",
                    addressed: Addressed::Signed("docdb"),
                    action: "DescribeDBEngineVersions",
                    params: &[],
                    expected_status: Some(200),
                    expected_code: None,
                    expected_body_contains: &[
                        "<Engine>docdb</Engine>",
                        "<DBParameterGroupFamily>docdb5.0</DBParameterGroupFamily>",
                    ],
                    expected_body_excludes: &[
                        "<Engine>mysql</Engine>",
                        "<Engine>postgres</Engine>",
                        "<Engine>neptune</Engine>",
                    ],
                },
                Call {
                    // An anonymous request addressed at the RDS endpoint host: the
                    // routing key comes from the host label instead of a credential
                    // scope, and must still resolve to the one service that owns it.
                    case: "rds DescribeDBEngineVersions addressed by host",
                    addressed: Addressed::Host("rds.us-east-1.amazonaws.com"),
                    action: "DescribeDBEngineVersions",
                    params: &[],
                    expected_status: Some(200),
                    expected_code: None,
                    expected_body_contains: &[],
                    expected_body_excludes: &[],
                },
                Call {
                    // The wire step the DescribeOptionGroups issue was filed on: a
                    // describe for an option group that does not exist. `500` JSON
                    // before; Amazon RDS's own modelled 404 in XML now.
                    case: "rds DescribeOptionGroups for a missing option group",
                    addressed: Addressed::Signed("rds"),
                    action: "DescribeOptionGroups",
                    params: &[("OptionGroupName", "no-such-option-group")],
                    expected_status: Some(404),
                    expected_code: Some("OptionGroupNotFoundFault"),
                    expected_body_contains: &["<ErrorResponse", "no-such-option-group"],
                    expected_body_excludes: &[],
                },
                Call {
                    // DocumentDB's model declares no `DescribeOptionGroups`, so its own
                    // envelope says so — proof this request reached DocumentDB and not
                    // Amazon RDS, which answers the row above for the very same action.
                    case: "docdb does not declare DescribeOptionGroups",
                    addressed: Addressed::Signed("docdb"),
                    action: "DescribeOptionGroups",
                    params: &[("OptionGroupName", "no-such-option-group")],
                    expected_status: Some(400),
                    expected_code: Some("InvalidAction"),
                    expected_body_contains: &[],
                    expected_body_excludes: &[],
                },
                Call {
                    case: "neptune does not declare DescribeOptionGroups",
                    addressed: Addressed::Signed("neptune"),
                    action: "DescribeOptionGroups",
                    params: &[("OptionGroupName", "no-such-option-group")],
                    expected_status: Some(400),
                    expected_code: Some("InvalidAction"),
                    expected_body_contains: &[],
                    expected_body_excludes: &[],
                },
                Call {
                    // The round trip through the shared front door, in two ordered rows
                    // against the one router: the create…
                    case: "rds CreateOptionGroup",
                    addressed: Addressed::Signed("rds"),
                    action: "CreateOptionGroup",
                    params: &[
                        ("OptionGroupName", "router-optiongroup"),
                        ("EngineName", "mysql"),
                        ("MajorEngineVersion", "8.0"),
                        ("OptionGroupDescription", "router-round-trip"),
                    ],
                    expected_status: Some(200),
                    expected_code: None,
                    expected_body_contains: &["<OptionGroupName>router-optiongroup</OptionGroupName>"],
                    expected_body_excludes: &[],
                },
                Call {
                    // …and the read back, which reports the members the create persisted.
                    case: "rds DescribeOptionGroups reads the created group back",
                    addressed: Addressed::Signed("rds"),
                    action: "DescribeOptionGroups",
                    params: &[("OptionGroupName", "router-optiongroup")],
                    expected_status: Some(200),
                    expected_code: None,
                    expected_body_contains: &[
                        "<OptionGroupName>router-optiongroup</OptionGroupName>",
                        "<EngineName>mysql</EngineName>",
                        "<MajorEngineVersion>8.0</MajorEngineVersion>",
                    ],
                    expected_body_excludes: &[],
                },
                Call {
                    case: "rds DescribeDBInstances for a missing instance",
                    addressed: Addressed::Signed("rds"),
                    action: "DescribeDBInstances",
                    params: &[("DBInstanceIdentifier", "no-such-instance")],
                    expected_status: Some(404),
                    expected_code: Some("DBInstanceNotFound"),
                    expected_body_contains: &["<ErrorResponse", "<RequestId>"],
                    expected_body_excludes: &[],
                },
                Call {
                    case: "rds DeleteDBInstance for a missing instance",
                    addressed: Addressed::Signed("rds"),
                    action: "DeleteDBInstance",
                    params: &[
                        ("DBInstanceIdentifier", "no-such-instance"),
                        ("SkipFinalSnapshot", "true"),
                    ],
                    expected_status: Some(404),
                    expected_code: Some("DBInstanceNotFound"),
                    expected_body_contains: &["<ErrorResponse", "<RequestId>"],
                    expected_body_excludes: &[],
                },
                Call {
                    case: "rds RebootDBInstance for a missing instance",
                    addressed: Addressed::Signed("rds"),
                    action: "RebootDBInstance",
                    params: &[("DBInstanceIdentifier", "no-such-instance")],
                    expected_status: Some(404),
                    expected_code: Some("DBInstanceNotFound"),
                    expected_body_contains: &["<ErrorResponse", "<RequestId>"],
                    expected_body_excludes: &[],
                },
                Call {
                    // The failure is attributed to the CLUSTER, not to the instance being
                    // created.
                    case: "rds CreateDBInstance naming a missing cluster",
                    addressed: Addressed::Signed("rds"),
                    action: "CreateDBInstance",
                    params: &[
                        ("DBInstanceIdentifier", "router-probe-member"),
                        ("Engine", "aurora-postgresql"),
                        ("DBInstanceClass", "db.r6g.large"),
                        ("DBClusterIdentifier", "no-such-cluster"),
                    ],
                    expected_status: Some(404),
                    expected_code: Some("DBClusterNotFoundFault"),
                    expected_body_contains: &["<ErrorResponse", "<RequestId>"],
                    expected_body_excludes: &[],
                },
                Call {
                    // The identifier is an ARN the CALLER composes for the resource the
                    // action is applied to, so a DB INSTANCE ARN that resolves to no
                    // resource of the account is refused on the MEMBER that carried it,
                    // echoing the value the request supplied. (A DB CLUSTER ARN that
                    // resolves to nothing is the cluster path's own not-found fault.)
                    case: "rds ApplyPendingMaintenanceAction on a missing resource ARN",
                    addressed: Addressed::Signed("rds"),
                    action: "ApplyPendingMaintenanceAction",
                    params: &[
                        (
                            "ResourceIdentifier",
                            "arn:aws:rds:us-east-1:000000000000:db:no-such-instance",
                        ),
                        ("ApplyAction", "system-update"),
                        ("OptInType", "immediate"),
                    ],
                    expected_status: Some(400),
                    expected_code: Some("InvalidParameterValue"),
                    expected_body_contains: &["<ErrorResponse", "<RequestId>"],
                    expected_body_excludes: &[],
                },
                Call {
                    case: "rds DescribeGlobalClusters for a missing global cluster",
                    addressed: Addressed::Signed("rds"),
                    action: "DescribeGlobalClusters",
                    params: &[("GlobalClusterIdentifier", "no-such-global-cluster")],
                    expected_status: Some(404),
                    expected_code: Some("GlobalClusterNotFoundFault"),
                    expected_body_contains: &["<ErrorResponse", "<RequestId>"],
                    expected_body_excludes: &[],
                },
                Call {
                    case: "rds DescribeEventSubscriptions for a missing subscription",
                    addressed: Addressed::Signed("rds"),
                    action: "DescribeEventSubscriptions",
                    params: &[("SubscriptionName", "no-such-subscription")],
                    expected_status: Some(404),
                    expected_code: Some("SubscriptionNotFound"),
                    expected_body_contains: &["<ErrorResponse", "<RequestId>"],
                    expected_body_excludes: &[],
                },
                Call {
                    // The same operation answers 200 for an unfiltered call (the rows
                    // above prove the catalog reads), so a 404 here is about the NAME.
                    case: "rds DescribeCertificates for a missing certificate",
                    addressed: Addressed::Signed("rds"),
                    action: "DescribeCertificates",
                    params: &[("CertificateIdentifier", "rds-ca-no-such-g9")],
                    expected_status: Some(404),
                    expected_code: Some("CertificateNotFound"),
                    expected_body_contains: &["<ErrorResponse", "<RequestId>"],
                    expected_body_excludes: &[],
                },
            ];

            for call in calls {
                let mut body = format!("Action={}&Version={API_VERSION}", call.action);
                for (name, value) in call.params {
                    body.push_str(&format!("&{name}={value}"));
                }
                let mut builder = http::Request::builder()
                    .method("POST")
                    .uri("http://localhost/")
                    .header("content-type", "application/x-www-form-urlencoded");
                builder = match call.addressed {
                    Addressed::Signed(signing_name) => builder
                        .header("host", "localhost")
                        .header(
                            "authorization",
                            format!(
                                "AWS4-HMAC-SHA256 \
                                 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
                                 SignedHeaders=host, Signature=not-verified"
                            ),
                        ),
                    Addressed::Host(host) => builder.header("host", host),
                };
                let request = builder
                    .body(bytes::Bytes::from(body))
                    .expect("a well-formed request");
                let response = runtime.block_on(router.dispatch(request));
                let status = response.status().as_u16();
                let rendered = String::from_utf8_lossy(response.body()).to_string();
                let case = call.case;

                // Neither routing failure may happen: `AmbiguousService` (500, several
                // claimants) nor `UnknownService` (404, none).
                assert!(
                    !rendered.contains("AmbiguousService") && !rendered.contains("UnknownService"),
                    "case {case:?}: the request must reach exactly one service, got \
                     {status} with {rendered}"
                );
                // Whatever the answer is, it is the target service's own awsQuery
                // envelope — XML, which a query-protocol client can parse — and never
                // the router's JSON `__type` document.
                assert!(
                    rendered.trim_start().starts_with('<') && !rendered.contains("__type"),
                    "case {case:?}: an awsQuery client must get XML, got {status} with \
                     {rendered}"
                );
                if let Some(expected) = call.expected_status {
                    assert_eq!(
                        status, expected,
                        "case {case:?}: the owning service must answer HTTP {expected}, \
                         got {status} with {rendered}"
                    );
                }
                if let Some(expected) = call.expected_code {
                    let code = rendered
                        .split_once("<Code>")
                        .and_then(|(_, rest)| rest.split_once("</Code>"))
                        .map(|(code, _)| code.to_string());
                    assert_eq!(
                        code.as_deref(),
                        Some(expected),
                        "case {case:?}: expected the error code {expected}, got {status} \
                         with {rendered}"
                    );
                }
                for expected in call.expected_body_contains {
                    assert!(
                        rendered.contains(expected),
                        "case {case:?}: expected {expected:?} in the owning service's \
                         answer, got {status} with {rendered}"
                    );
                }
                for unexpected in call.expected_body_excludes {
                    assert!(
                        !rendered.contains(unexpected),
                        "case {case:?}: {unexpected:?} belongs to another service's answer \
                         and must not be in this one, got {status} with {rendered}"
                    );
                }
            }
        })
        .expect("spawn the routing thread")
        .join()
        .expect("RDS, DocumentDB and Neptune must route apart");
}

/// The SigV4 `Authorization` header of a request signed for `signing_name`, as every AWS
/// SDK sends it. The signature is never checked: the claim predicate reads only the
/// credential scope.
pub(crate) fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// How a row of the claim table below names the service it is for: the same two signals
/// [`Addressed`] carries, plus the presigned form of a credential scope, which an SDK
/// puts in the query string instead of a header.
#[derive(Debug, Clone, Copy)]
enum Claiming {
    /// A SigV4 `Authorization` scope naming this routing key.
    Signed(&'static str),
    /// A presigned URL's `X-Amz-Credential` query parameter naming `rds`.
    Presigned,
    /// No credentials at all, so the `Host` label resolves the key.
    Anonymous,
}

/// The claim table's own rule, one layer BELOW the dispatch table above: it asks each
/// service's [`Service::can_handle`] directly, so a row pins both that the right service
/// claims a request and that NO other does — the condition the router turns into
/// `500 AmbiguousService` when it fails.
///
/// Every `awsQuery` request is `POST /`: none of the three models binds a
/// `smithy.api#http` trait, so the path carries no discriminator and the route key
/// decides alone. The rows span the three signals that resolve it, the three keys, and a
/// `Host` that agrees with, disagrees with or says nothing about the target.
#[test]
fn exactly_one_service_claims_each_request_for_the_rds_key() {
    use smithy_server_runtime::Service;

    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let rds = service_com_amazonaws_rds_amazonrdsv19::EmulatedService::new(Arc::clone(&engine));
    let docdb = service_com_amazonaws_docdb_amazonrdsv19::EmulatedService::new(Arc::clone(&engine));
    let neptune = service_com_amazonaws_neptune_amazonrdsv19::EmulatedService::new(engine);

    /// A presigned credential scope naming `rds`, with its `/` separators encoded.
    const PRESIGNED: &str = "X-Amz-Credential=AKIDEXAMPLE%2F20260101%2Fus-east-1%2Frds%2Faws4_request";

    // (case, how the request is addressed, its Host, the services that must claim it, by
    // routing key)
    let rows: [(&str, Claiming, &str, &[&str]); 10] = [
        (
            "a signed request on the rds host",
            Claiming::Signed("rds"),
            "rds.us-east-1.amazonaws.com",
            &["rds"],
        ),
        (
            "a signed request on a bare emulator endpoint",
            Claiming::Signed("rds"),
            "localhost:9297",
            &["rds"],
        ),
        (
            "a signed request on the DocumentDB console's host: the scope decides",
            Claiming::Signed("rds"),
            "docdb.us-east-1.amazonaws.com",
            &["rds"],
        ),
        (
            "a presigned request in the query string",
            Claiming::Presigned,
            "rds.eu-west-2.amazonaws.com",
            &["rds"],
        ),
        (
            "an anonymous request addressed to the rds host",
            Claiming::Anonymous,
            "rds.us-east-1.amazonaws.com",
            &["rds"],
        ),
        (
            "a request signed for DocumentDB's own key",
            Claiming::Signed("docdb"),
            "rds.us-east-1.amazonaws.com",
            &["docdb"],
        ),
        (
            "an anonymous request addressed to the DocumentDB host",
            Claiming::Anonymous,
            "docdb.us-east-1.amazonaws.com",
            &["docdb"],
        ),
        (
            "a request signed for Neptune's own key",
            Claiming::Signed("neptune"),
            "rds.us-east-1.amazonaws.com",
            &["neptune"],
        ),
        (
            "an anonymous request addressed to the Neptune host",
            Claiming::Anonymous,
            "neptune.us-east-1.amazonaws.com",
            &["neptune"],
        ),
        (
            // Claimed by nobody, which is the router's `404 UnknownService` and NOT an
            // ambiguity.
            "an anonymous request addressed to an unrelated host",
            Claiming::Anonymous,
            "sqs.us-east-1.amazonaws.com",
            &[],
        ),
    ];

    for (case, claiming, host, expected) in rows {
        let uri = match claiming {
            Claiming::Presigned => format!("/?{PRESIGNED}"),
            _ => "/".to_string(),
        };
        let mut builder = http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("host", host);
        if let Claiming::Signed(signing_name) = claiming {
            builder = builder.header("authorization", authorization(signing_name));
        }
        let parts = builder
            .body(bytes::Bytes::new())
            .expect("a well-formed request")
            .into_parts()
            .0;
        let claimants: Vec<&str> = [
            ("rds", rds.can_handle(&parts)),
            ("docdb", docdb.can_handle(&parts)),
            ("neptune", neptune.can_handle(&parts)),
        ]
        .into_iter()
        .filter_map(|(key, claimed)| claimed.then_some(key))
        .collect();
        assert_eq!(
            claimants,
            expected.to_vec(),
            "case {case:?} (POST / to host {host}, {claiming:?}): expected it to be \
             claimed by exactly {expected:?}, got {claimants:?}"
        );
    }
}
