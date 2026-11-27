// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the awsQuery surface this front door shares with the Amazon
//! DocumentDB and Amazon Neptune control planes: the error ENVELOPE every refusal is
//! wrapped in, the ORDER in which a request's addressed resource and its referenced
//! resources are resolved, and the two reports of published reference data that span
//! every engine this door serves.
//!
//! One Query front door serves three control planes — this model's wire identity
//! (`endpointPrefix` / `sigv4Name` `rds`, `awsQuery`, version `2014-10-31`) is published
//! identically by the DocumentDB and Neptune models, so a request signed for `rds` reaches
//! THIS service whichever of the three APIs the caller compiled against
//! (`src/service.rs`). Every rule below therefore has to hold for a caller of any of them,
//! which is what these tables span.
//!
//! The rules pinned here, each over a SET of inputs rather than one request:
//!
//! * every awsQuery error response carries a `<RequestId>` in its `<ErrorResponse>`
//!   envelope — spanned over the layers that can produce one (a modelled fault from a
//!   handler, a handler's own refusal, a framework refusal for an absent required member,
//!   an unroutable `Action`, an API version this door does not serve), and the ids are
//!   distinct per response while a SUCCESS keeps its `ResponseMetadata/RequestId`;
//! * the resource a request ADDRESSES is resolved before any rule about how its other
//!   members combine — spanned over every combination of `DeleteDBInstance`'s two snapshot
//!   members, against an instance that does not exist and against one that does;
//! * a create resolves every resource it REFERENCES before admitting a record — spanned
//!   over `CreateDBInstance`'s `DBClusterIdentifier` on several engines and identifier
//!   forms, with the no-reference row as the control;
//! * `DefaultOnly` either names the one default version of the engine asked for or is
//!   refused with `InvalidParameterCombination` — spanned over EVERY engine this door's
//!   catalogue reports, read from the catalogue itself;
//! * an engine version reports the CA certificates it can be launched with, and they are
//!   the ones `DescribeCertificates` reports that have not expired — spanned over every
//!   engine, with the certificate catalogue as the source of truth;
//! * an orderable option of an engine this door serves for another control plane is a
//!   cluster member — spanned over every engine and every class the report offers.

use std::thread::sleep;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_rds_amazonrdsv19::state::{
    CertificateKey, CA_CERTIFICATES, DEFAULT_CA_CERTIFICATE_IDENTIFIER, STATUS_TRANSITION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, emulator, expect_error, expect_ok, subtrees, text, texts};

/// The engines this front door reports on behalf of ANOTHER control plane.
const ENGINES_OF_ANOTHER_CONTROL_PLANE: [&str; 2] = ["docdb", "neptune"];

/// The one engine this door resolves no DEFAULT engine version for: recorded behaviour on
/// the document engine is the `InvalidParameterCombination` refusal, while the graph engine
/// publishes a default of its own like every other engine here.
const ENGINE_WITH_NO_DEFAULT_VERSION: &str = "docdb";

/// The engines whose instances are CLUSTER members — the Aurora engines plus the two
/// above — as `src/ops/describe_db_engine_versions.rs` names them.
fn is_cluster_engine(engine: &str) -> bool {
    engine.starts_with("aurora") || ENGINES_OF_ANOTHER_CONTROL_PLANE.contains(&engine)
}

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// The query parameters of one call, owned so a table row can hold them.
type Params = Vec<(&'static str, String)>;

/// One row of the envelope table: the case, the action, its parameters, and the HTTP
/// status and wire error `<Code>` the request must be refused with.
type RefusalRow = (&'static str, &'static str, Params, u16, &'static str);

// ---------------------------------------------------------------------------
// The awsQuery error envelope
// ---------------------------------------------------------------------------

/// The `<RequestId>` of an `<ErrorResponse>` envelope: the element AFTER `</Error>` and
/// INSIDE `</ErrorResponse>`, which is where the envelope's own request id lives (a
/// `<RequestId>` anywhere else would belong to some error shape's members).
fn envelope_request_id(case: &str, body: &str) -> String {
    assert!(
        body.contains("<ErrorResponse"),
        "case {case:?}: expected an <ErrorResponse> envelope, got {body}"
    );
    let error_close = body
        .rfind("</Error>")
        .unwrap_or_else(|| panic!("case {case:?}: expected a closed <Error>, got {body}"));
    let envelope_close = body.rfind("</ErrorResponse>").unwrap_or_else(|| {
        panic!("case {case:?}: expected a closed <ErrorResponse>, got {body}")
    });
    let tail = &body[error_close..envelope_close];
    assert!(
        tail.contains("<RequestId>"),
        "case {case:?}: expected <RequestId> between </Error> and </ErrorResponse>, got {body}"
    );
    let request_id = text(body, "RequestId");
    let groups: Vec<usize> = request_id.split('-').map(str::len).collect();
    assert_eq!(
        groups,
        vec![8, 4, 4, 4, 12],
        "case {case:?}: a request id is an 8-4-4-4-12 hexadecimal id, got {request_id:?}"
    );
    assert!(
        request_id
            .chars()
            .all(|character| character == '-' || character.is_ascii_hexdigit()),
        "case {case:?}: a request id is hexadecimal, got {request_id:?}"
    );
    request_id
}

#[tokio::test]
async fn every_awsquery_error_envelope_carries_its_own_request_id() {
    let router = emulator();

    // (case, action, parameters, status, wire error code) — one row per LAYER that can
    // answer an error, because the element belongs to the envelope and not to whichever
    // layer produced the fault.
    let rows: Vec<RefusalRow> = vec![
        (
            "a modelled fault from a handler",
            "DescribeDBInstances",
            params!["DBInstanceIdentifier" => "emu-no-such-instance"].to_vec(),
            404,
            "DBInstanceNotFound",
        ),
        (
            "a modelled fault on a delete",
            "DeleteDBInstance",
            params!["DBInstanceIdentifier" => "emu-no-such-instance"].to_vec(),
            404,
            "DBInstanceNotFound",
        ),
        (
            "a modelled fault naming a resource the request references",
            "CreateDBInstance",
            params![
                "DBInstanceIdentifier" => "emu-envelope-1",
                "Engine" => "docdb",
                "DBInstanceClass" => "db.r5.large",
                "DBClusterIdentifier" => "emu-no-such-cluster",
            ]
            .to_vec(),
            404,
            "DBClusterNotFoundFault",
        ),
        (
            "a handler's own refusal",
            "DescribeDBEngineVersions",
            params!["DefaultOnly" => "true"].to_vec(),
            400,
            "InvalidParameterCombination",
        ),
        (
            // A `@required` member the request did not carry is answered under the Query
            // API code this front door gives a parameter it was not given acceptably.
            "a framework refusal for an absent required member",
            "CreateDBInstance",
            params!["Engine" => "mysql"].to_vec(),
            400,
            "InvalidParameterValue",
        ),
        (
            "an Action this door models no operation for",
            "NotAnActionAtAll",
            Vec::new(),
            400,
            "InvalidAction",
        ),
    ];

    let mut seen: Vec<(&str, String)> = Vec::new();
    for (case, action, parameters, status, code) in rows {
        let body = expect_error(case, call(&router, action, &parameters).await, status, code);
        let request_id = envelope_request_id(case, &body);
        if let Some((other, _)) = seen.iter().find(|(_, other)| *other == request_id) {
            panic!("case {case:?}: answered the same request id as case {other:?}");
        }
        seen.push((case, request_id));
    }

    // The API-version refusal this service raises itself (its `Version` names an API this
    // door does not serve) answers in the same envelope. The shared `call` helper always
    // sends this model's own version, so this one row builds the envelope itself.
    let request = http::Request::builder()
        .method("POST")
        .uri("https://rds.amazonaws.com/")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{}/rds/aws4_request, \
                 SignedHeaders=host, Signature=unverified",
                crate::db_instances::REGION
            ),
        )
        .body(bytes::Bytes::from(
            "Action=DescribeDBInstances&Version=2099-01-01",
        ))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8(response.body().to_vec()).expect("a UTF-8 response body");
    assert_eq!(
        status, 400,
        "an API version this door does not serve is refused, got {status} with {body}"
    );
    envelope_request_id("an API version this door does not serve", &body);

    // A SUCCESS is untouched: its request id is the `ResponseMetadata/RequestId` the
    // framework writes, and nothing added a second one or an error envelope to it.
    let body = expect_ok(
        "a successful read",
        call(&router, "DescribeDBInstances", &params!["MaxRecords" => 20]).await,
    );
    assert!(
        !body.contains("<ErrorResponse"),
        "a success must carry no error envelope, got {body}"
    );
    assert!(
        body.contains("<ResponseMetadata><RequestId>"),
        "a success carries its request id in ResponseMetadata, got {body}"
    );
    assert_eq!(
        texts(&body, "RequestId").len(),
        1,
        "a success carries exactly one request id, got {body}"
    );
}

// ---------------------------------------------------------------------------
// The order a request's resources are resolved in
// ---------------------------------------------------------------------------

/// `Action=Version=2014-10-31` plus the snapshot members of one `DeleteDBInstance` row.
fn delete_params(identifier: &str, snapshot: &[(&str, &str)]) -> Vec<(&'static str, String)> {
    let mut parameters: Vec<(&'static str, String)> =
        vec![("DBInstanceIdentifier", identifier.to_string())];
    for (name, value) in snapshot {
        let name: &'static str = match *name {
            "SkipFinalSnapshot" => "SkipFinalSnapshot",
            "FinalDBSnapshotIdentifier" => "FinalDBSnapshotIdentifier",
            other => panic!("unexpected snapshot member {other}"),
        };
        parameters.push((name, (*value).to_string()));
    }
    parameters
}

#[tokio::test]
async fn the_instance_a_delete_addresses_is_resolved_before_its_parameter_combinations() {
    let router = emulator();

    // Every combination of the two snapshot members `DeleteDBInstance` carries, including
    // the two the documentation forbids. Against an instance that does not exist, ALL of
    // them answer the modelled `DBInstanceNotFoundFault`: there is no deletion for a
    // snapshot parameter to qualify, and a caller deleting if-exists branches on
    // 404/`DBInstanceNotFound`. (The DocumentDB and Neptune models carry neither member at
    // all, so for their callers a refusal naming one names nothing they sent.)
    let combinations: [(&str, &[(&str, &str)]); 4] = [
        ("neither snapshot member", &[]),
        ("skipping the final snapshot", &[("SkipFinalSnapshot", "true")]),
        (
            "naming a final snapshot",
            &[("FinalDBSnapshotIdentifier", "emu-final-1")],
        ),
        (
            "both, which the documentation forbids",
            &[
                ("SkipFinalSnapshot", "true"),
                ("FinalDBSnapshotIdentifier", "emu-final-2"),
            ],
        ),
    ];

    for (case, snapshot) in combinations {
        let body = expect_error(
            case,
            call(
                &router,
                "DeleteDBInstance",
                &delete_params("emu-absent-instance", snapshot),
            )
            .await,
            404,
            "DBInstanceNotFound",
        );
        let message = text(&body, "Message");
        assert!(
            message.contains("emu-absent-instance"),
            "case {case:?}: the refusal must name the instance it could not resolve, got \
             {message:?}"
        );
    }

    // On an instance that DOES exist the combination rules still decide, so resolving the
    // instance first did not drop them.
    expect_ok(
        "create the instance the rows below address",
        call(
            &router,
            "CreateDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-present-instance",
                "Engine" => "mysql",
                "DBInstanceClass" => "db.t3.micro",
            ],
        )
        .await,
    );
    sleep(STATUS_TRANSITION_DELAY + Duration::from_millis(200));

    for (case, snapshot) in [
        ("both on an instance that exists", combinations[3].1),
        ("neither on an instance that exists", combinations[0].1),
    ] {
        let body = expect_error(
            case,
            call(
                &router,
                "DeleteDBInstance",
                &delete_params("emu-present-instance", snapshot),
            )
            .await,
            400,
            "InvalidParameterCombination",
        );
        let message = text(&body, "Message");
        assert!(
            message.contains("FinalDBSnapshotIdentifier"),
            "case {case:?}: the refusal must name the member combination it refused, got \
             {message:?}"
        );
    }
    let body = expect_ok(
        "a well-formed combination on an instance that exists",
        call(
            &router,
            "DeleteDBInstance",
            &delete_params("emu-present-instance", combinations[1].1),
        )
        .await,
    );
    assert_eq!(
        text(&body, "DBInstanceStatus"),
        "deleting",
        "the delete that passes both checks must be served, got {body}"
    );
}

#[tokio::test]
async fn an_instance_is_not_admitted_into_a_cluster_that_does_not_exist() {
    let router = emulator();

    // (case, engine, instance class, the cluster the request names). The cluster
    // identifier is the input the rule generalises over — its spelling, its length and the
    // engine of the instance that would join it all vary, and none of them makes a cluster
    // exist.
    let rows: [(&str, &str, &str, &str); 5] = [
        ("a DocumentDB cluster member", "docdb", "db.r5.large", "emu-docdb-cluster"),
        ("a Neptune cluster member", "neptune", "db.r5.large", "emu-neptune-cluster"),
        (
            "an Aurora cluster member",
            "aurora-mysql",
            "db.r5.large",
            "emu-aurora-cluster",
        ),
        (
            "a mixed-case cluster identifier",
            "postgres",
            "db.t3.micro",
            "Emu-Mixed-Case-Cluster",
        ),
        (
            "an RDS engine naming a Multi-AZ DB cluster",
            "mysql",
            "db.t3.micro",
            "emu-multi-az-cluster",
        ),
    ];

    for (index, (case, engine, class, cluster)) in rows.iter().enumerate() {
        let identifier = format!("emu-cluster-member-{index}");
        let body = expect_error(
            case,
            call(
                &router,
                "CreateDBInstance",
                &params![
                    "DBInstanceIdentifier" => identifier,
                    "Engine" => engine,
                    "DBInstanceClass" => class,
                    "DBClusterIdentifier" => cluster,
                ],
            )
            .await,
            404,
            "DBClusterNotFoundFault",
        );
        let message = text(&body, "Message");
        assert!(
            message.contains(cluster),
            "case {case:?}: the refusal must name the cluster it could not resolve, got \
             {message:?}"
        );
        // Refused means NOT created: a later describe must not find a phantom member.
        expect_error(
            case,
            call(
                &router,
                "DescribeDBInstances",
                &params!["DBInstanceIdentifier" => identifier],
            )
            .await,
            404,
            "DBInstanceNotFound",
        );
    }

    // The control: the same create naming NO cluster is admitted, so the refusal is about
    // the reference it could not resolve and not about creating an instance.
    let body = expect_ok(
        "the same create naming no cluster",
        call(
            &router,
            "CreateDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-standalone-instance",
                "Engine" => "mysql",
                "DBInstanceClass" => "db.t3.micro",
            ],
        )
        .await,
    );
    assert_eq!(
        text(&body, "DBInstanceIdentifier"),
        "emu-standalone-instance",
        "an instance that references nothing is created, got {body}"
    );
}

// ---------------------------------------------------------------------------
// The engine reports, over every engine this door serves
// ---------------------------------------------------------------------------

/// Every engine this door's catalogue reports a version for, read from the report itself
/// so the tables below span the catalogue rather than a copy of it.
async fn catalogue_engines(router: &MultiServiceRouter) -> Vec<String> {
    let body = expect_ok(
        "the whole engine catalogue",
        call(
            router,
            "DescribeDBEngineVersions",
            &params!["IncludeAll" => "true", "MaxRecords" => 100],
        )
        .await,
    );
    let mut engines: Vec<String> = Vec::new();
    for version in subtrees(&body, "DBEngineVersion") {
        // An upgrade target carries its own `<Engine>`, so read the entry's members with
        // its `<ValidUpgradeTarget>` subtrees removed.
        let mut scalars = version.clone();
        for target in subtrees(&version, "ValidUpgradeTarget") {
            scalars = scalars.replace(&target, "");
        }
        let engine = text(&scalars, "Engine");
        if !engines.contains(&engine) {
            engines.push(engine);
        }
    }
    assert!(
        engines.len() > 5,
        "the catalogue must report several engines, got {engines:?}"
    );
    engines
}

#[tokio::test]
async fn default_only_names_one_default_version_or_is_refused() {
    let router = emulator();

    for engine in catalogue_engines(&router).await {
        let case = format!("DefaultOnly for {engine}");
        let answer = call(
            &router,
            "DescribeDBEngineVersions",
            &params!["Engine" => engine, "DefaultOnly" => "true"],
        )
        .await;
        if engine == ENGINE_WITH_NO_DEFAULT_VERSION {
            let body = expect_error(&case, answer, 400, "InvalidParameterCombination");
            assert!(
                text(&body, "Message").contains("default engine version"),
                "case {case:?}: the refusal must say no default engine version resolved, got \
                 {body}"
            );
            // The versions themselves are still reported: only the DEFAULT of them is
            // not something this door resolves.
            let body = expect_ok(
                &case,
                call(
                    &router,
                    "DescribeDBEngineVersions",
                    &params!["Engine" => engine, "IncludeAll" => "true"],
                )
                .await,
            );
            assert!(
                !subtrees(&body, "DBEngineVersion").is_empty(),
                "case {case:?}: this door reports that control plane's engine versions, got \
                 {body}"
            );
            continue;
        }
        let body = expect_ok(&case, answer);
        let reported = subtrees(&body, "DBEngineVersion");
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected exactly the engine's default version, got {body}"
        );
    }
}

/// Whether a published certificate validity has passed.
fn has_expired(valid_till: &str) -> bool {
    let till = ::aws_smithy_types::DateTime::from_str(
        valid_till,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|_| panic!("the catalogue's {valid_till:?} is an ISO-8601 instant"));
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_secs() as i64);
    till.secs() <= now
}

#[tokio::test]
async fn an_engine_version_reports_the_certificates_it_can_be_launched_with() {
    let router = emulator();

    // The rule's source is the certificate catalogue `DescribeCertificates` reports: an
    // engine version can be launched with the certificates in it that have NOT expired,
    // so the two reports cannot name different certificates. WHICH of them an entry
    // publishes is the ENGINE's: the still-valid authorities an engine is offered, the
    // elliptic-curve one first and then the RSA ones from the longest key to the shortest,
    // and the elliptic-curve authority only for an Aurora engine.
    let certificates = expect_ok(
        "the certificate catalogue",
        call(&router, "DescribeCertificates", &params!["MaxRecords" => 20]).await,
    );
    let reported: Vec<String> = texts(&certificates, "CertificateIdentifier");
    let mut still_valid: Vec<(u8, u32, String)> = CA_CERTIFICATES
        .iter()
        .filter(|certificate| !has_expired(certificate.valid_till))
        .map(|certificate| match certificate.key {
            CertificateKey::EllipticCurve(bits) => (0, bits, certificate.identifier.to_string()),
            CertificateKey::Rsa(bits) => (1, bits, certificate.identifier.to_string()),
        })
        .collect();
    still_valid.sort_by(|left, right| left.0.cmp(&right.0).then(right.1.cmp(&left.1)));
    let published = |engine: &str| -> Vec<String> {
        still_valid
            .iter()
            .filter(|(kind, _, _)| *kind == 1 || engine.starts_with("aurora"))
            .map(|(_, _, identifier)| identifier.clone())
            .collect()
    };
    let every_authority: Vec<String> = published("aurora-mysql");
    let rsa_only: Vec<String> = published("mysql");
    assert!(
        rsa_only.len() < every_authority.len() && every_authority.len() == reported.len(),
        "the catalogue publishes no expired authority, so an Aurora engine is offered all \
         of it and an engine serving an RSA certificate fewer, or this rule tests nothing: \
         reported {reported:?}, offered to Aurora {every_authority:?}, offered to an \
         RSA-only engine {rsa_only:?}"
    );
    assert!(
        every_authority.contains(&DEFAULT_CA_CERTIFICATE_IDENTIFIER.to_string()),
        "the certificate new instances get must be one an engine version can be launched \
         with, got {every_authority:?}"
    );

    for engine in catalogue_engines(&router).await {
        let case = format!("the certificates of {engine}");
        let expected = published(&engine);
        let body = expect_ok(
            &case,
            call(
                &router,
                "DescribeDBEngineVersions",
                &params!["Engine" => engine, "IncludeAll" => "true"],
            )
            .await,
        );
        for version in subtrees(&body, "DBEngineVersion") {
            // The member is a list, so its entries arrive member-wrapped; read them by
            // the element the list's member publishes.
            let wrapped = subtrees(&version, "SupportedCACertificateIdentifiers");
            assert_eq!(
                wrapped.len(),
                1,
                "case {case:?}: one entry reports one certificate list, got {version}"
            );
            let listed = texts(&wrapped[0], "member");
            assert_eq!(
                listed, expected,
                "case {case:?}: an engine version reports the certificates this engine can \
                 be launched with, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_orderable_option_of_another_control_planes_engine_is_a_cluster_member() {
    let router = emulator();

    for engine in catalogue_engines(&router).await {
        let case = format!("the orderable options of {engine}");
        let body = expect_ok(
            &case,
            call(
                &router,
                "DescribeOrderableDBInstanceOptions",
                &params!["Engine" => engine, "MaxRecords" => 1000],
            )
            .await,
        );
        let options = subtrees(&body, "OrderableDBInstanceOption");
        assert!(
            !options.is_empty(),
            "case {case:?}: an engine whose versions this door reports has orderable \
             options, got {body}"
        );
        for option in &options {
            let class = text(option, "DBInstanceClass");
            let supports_clusters = text(option, "SupportsClusters");
            // An instance of a CLUSTER engine — the Aurora engines, and the DocumentDB and
            // Neptune engines this door serves for another control plane — is only ever a
            // member of its cluster, whatever its class, which recorded behaviour reports
            // this member true for. On the RDS DB instance engines the Multi-AZ DB cluster
            // deployment is offered for MySQL and PostgreSQL on the general-purpose and
            // memory-optimized classes that carry local NVMe SSD storage (`db.m5d`,
            // `db.m6gd`, `db.r5d`, `db.r6gd`), and on neither a burstable (`db.t*`) class
            // nor a compute-optimized one.
            let family = class.strip_prefix("db.").unwrap_or(&class);
            let purpose = family.chars().next().unwrap_or_default();
            let has_local_nvme = family
                .split('.')
                .next()
                .unwrap_or_default()
                .trim_start_matches(|character: char| character.is_ascii_alphabetic())
                .contains('d');
            let expected = is_cluster_engine(&engine)
                || (matches!(engine.as_str(), "mysql" | "postgres")
                    && matches!(purpose, 'm' | 'r' | 'x' | 'z')
                    && has_local_nvme);
            assert_eq!(
                supports_clusters,
                expected.to_string(),
                "case {case:?}: class {class:?} reported SupportsClusters \
                 {supports_clusters:?}"
            );
            if is_cluster_engine(&engine) {
                assert_eq!(
                    text(option, "Vpc"),
                    "true",
                    "case {case:?}: a cluster member is a VPC offering, got {option}"
                );
            }
        }
    }
}
