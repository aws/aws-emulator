// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two catalog reads this service answers out of the published
//! catalogs in `src/state.rs` — `DescribeCertificates` and `DescribeEventCategories` —
//! and for the API version its Query envelope must name.
//!
//! (`DescribeDBEngineVersions` and `DescribeOrderableDBInstanceOptions` are the other two
//! reports of published reference data; they have their own suites, `db_engine_versions`
//! and `orderable_db_instance_options`. What this suite adds for them is the ONE rule they
//! share with every other action: the `Version` the Query envelope names must be the one
//! this model declares.)
//!
//! Every call goes over the wire through the same [`MultiServiceRouter`] the production
//! router uses: a real `awsQuery` form POST in, the XML the emulator answers with out.
//! The harness (`emulator`, `call`, `expect_ok`, `expect_error` and the element readers)
//! is the sibling [`crate::db_instances`] suite's, reused rather than copied.
//!
//! The rules these tables pin, and the classes they span:
//!
//! * a catalog read reports the catalog, narrowed by the members the model documents as
//!   narrowing it — spanned over the whole catalog, each entry on its own, a non-canonical
//!   spelling of an entry's key, and the published `MaxRecords` bound;
//! * a value outside a published set is refused with the code AWS answers for it, never
//!   served as an empty list — spanned over an unknown CA certificate, four forms of
//!   unknown event source type, an unsupported filter on each operation, and
//!   `DefaultOnly` with no engine whose default version could be resolved;
//! * the engine reports of this front door carry the engines of EVERY control plane it
//!   serves — spanned over the Amazon DocumentDB and Amazon Neptune engines, which this
//!   model's own `Engine` member does not list;
//! * the Query envelope's `Version` must be the one this model declares — spanned over
//!   that version, an older one, a future one, a value that is not a date, and no version
//!   at all.

use service_com_amazonaws_rds_amazonrdsv19::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, emulator, expect_error, expect_ok, optional_text, subtrees, text, texts, REGION,
};

/// The API version this model declares, which every request below names unless it is
/// testing what happens when it names another.
const API_VERSION: &str = "2014-10-31";

/// A `DescribeEventCategories` call that names no `SourceType`, so it reports every
/// published map.
const NO_SOURCE_TYPE: [(&str, &str); 0] = [];

/// The query parameters one table row sends beyond `Action` and `Version`.
type Params = Vec<(&'static str, &'static str)>;

/// One row of a catalog-read table: the case name, the row's parameters, and the values
/// the answer must carry, in order.
type CatalogRow = (&'static str, Params, Vec<&'static str>);

/// A [`CatalogRow`] whose case name and expected values are built at run time from the
/// catalog itself.
type OwnedCatalogRow = (String, Params, Vec<String>);

/// One row of a refusal table: the case name and the row's parameters.
type RefusalRow = (&'static str, Params);

/// One `awsQuery` call whose envelope names `version` (or carries no `Version` at all),
/// returning the HTTP status, the `content-type` and the body.
///
/// The sibling suite's `call` always sends this model's own version, which is what every
/// other test wants; the version is a parameter here because it is what these rows span.
async fn call_versioned(
    router: &MultiServiceRouter,
    action: &str,
    version: Option<&str>,
) -> (u16, String, String) {
    let mut body = format!("Action={action}");
    if let Some(version) = version {
        body.push_str(&format!("&Version={version}"));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("https://rds.amazonaws.com/")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/rds/aws4_request, \
                 SignedHeaders=host, Signature=unverified"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .map(|value| value.to_str().unwrap_or_default().to_string())
        .unwrap_or_default();
    let body = String::from_utf8(response.body().to_vec()).expect("a UTF-8 response body");
    (status, content_type, body)
}

#[tokio::test]
async fn describe_certificates_reports_the_published_ca_catalog() {
    let router = emulator();

    // (case, parameters, the certificate identifiers the answer must carry, in order)
    let rows: Vec<CatalogRow> = vec![
        (
            "every published CA",
            vec![],
            state::CA_CERTIFICATES
                .iter()
                .map(|certificate| certificate.identifier)
                .collect(),
        ),
        (
            "one CA by identifier",
            vec![("CertificateIdentifier", "rds-ca-rsa4096-g1")],
            vec!["rds-ca-rsa4096-g1"],
        ),
        (
            // "This parameter is not case sensitive."
            "the identifier is not case sensitive",
            vec![("CertificateIdentifier", "RDS-CA-ECC384-G1")],
            vec!["rds-ca-ecc384-g1"],
        ),
        (
            "MaxRecords at its documented minimum",
            vec![("MaxRecords", "20")],
            state::CA_CERTIFICATES
                .iter()
                .map(|certificate| certificate.identifier)
                .collect(),
        ),
    ];

    for (case, params, expected) in rows {
        let body = expect_ok(case, call(&router, "DescribeCertificates", &params).await);
        let reported = texts(&body, "CertificateIdentifier");
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the certificates {expected:?}, got {reported:?} in {body}"
        );
        let default = text(&body, "DefaultCertificateForNewLaunches");
        assert_eq!(
            default,
            state::DEFAULT_CA_CERTIFICATE_IDENTIFIER,
            "case {case:?}: no override is set here, so the report must name the system \
             default CA, got {default:?} in {body}"
        );
        assert_eq!(
            optional_text(&body, "Marker"),
            None,
            "case {case:?}: the whole catalog fits one page, so no marker is due: {body}"
        );
        for certificate in subtrees(&body, "Certificate") {
            let identifier = text(&certificate, "CertificateIdentifier");
            let arn = text(&certificate, "CertificateArn");
            // A CA certificate is published BY the service, so its ARN names no owning
            // account: the account field is empty, unlike every other RDS resource ARN.
            assert_eq!(
                arn,
                format!("arn:aws:rds:{REGION}::cert:{identifier}"),
                "case {case:?}: expected {identifier} to carry its service-owned ARN in the \
                 signed region, got {arn:?}"
            );
            let (valid_from, valid_till) = (
                text(&certificate, "ValidFrom"),
                text(&certificate, "ValidTill"),
            );
            assert!(
                valid_from < valid_till,
                "case {case:?}: expected {identifier} to be valid from before it expires, \
                 got {valid_from} .. {valid_till}"
            );
            assert_eq!(
                text(&certificate, "CertificateType"),
                "CA",
                "case {case:?}: expected {identifier} to be a CA certificate: {certificate}"
            );
        }
    }

    // A value outside the published set: the modelled 404, not an empty list. An authority
    // the service has retired — its certificate expired, so it is no longer provided — is
    // outside that set exactly like an identifier that never named one.
    for (case, identifier) in [
        ("an identifier no published CA carries", "rds-ca-2042"),
        ("an authority whose certificate has expired", "rds-ca-2019"),
    ] {
        expect_error(
            case,
            call(
                &router,
                "DescribeCertificates",
                &[("CertificateIdentifier", identifier)],
            )
            .await,
            404,
            "CertificateNotFound",
        );
    }

    // `Filters` is documented as "not currently supported", so a filter is refused
    // rather than dropped.
    expect_error(
        "a filter this operation does not support",
        call(
            &router,
            "DescribeCertificates",
            &[
                ("Filters.Filter.1.Name", "certificate-id"),
                ("Filters.Filter.1.Values.Value.1", "rds-ca-2019"),
            ],
        )
        .await,
        400,
        "InvalidParameterValue",
    );
}

#[tokio::test]
async fn describe_event_categories_reports_the_published_source_types() {
    let router = emulator();

    let all_source_types: Vec<String> = state::EVENT_CATEGORIES
        .iter()
        .map(|(source_type, _)| (*source_type).to_string())
        .collect();

    // (case, parameters, the source types the answer must carry, in order)
    let mut rows: Vec<OwnedCatalogRow> = vec![(
        "no source type: every published one".to_string(),
        vec![],
        all_source_types.clone(),
    )];
    // One row per published source type: each must report its own categories alone.
    for (source_type, _) in state::EVENT_CATEGORIES {
        rows.push((
            format!("the {source_type} source type alone"),
            vec![("SourceType", source_type)],
            vec![(*source_type).to_string()],
        ));
    }

    for (case, params, expected) in rows {
        let body = expect_ok(&case, call(&router, "DescribeEventCategories", &params).await);
        let reported = texts(&body, "SourceType");
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the source types {expected:?}, got {reported:?} in {body}"
        );
        for map in subtrees(&body, "EventCategoriesMap") {
            let source_type = text(&map, "SourceType");
            let expected_categories = state::event_categories_of(&source_type)
                .unwrap_or_else(|| panic!("case {case:?}: {source_type} is not published"));
            let reported_categories = texts(&map, "EventCategory");
            assert_eq!(
                reported_categories, expected_categories,
                "case {case:?}: expected {source_type} to carry {expected_categories:?}, got \
                 {reported_categories:?}"
            );
        }
    }

    // A source type this report publishes no categories for is refused with the code AWS
    // answers for a parameter value it does not accept, in the one fixed sentence that names
    // the source types the front door accepts (`rds_event_source_types::EVENT_SOURCE_TYPES`,
    // a wider set than the ones this report carries a map for) and that echoes nothing of
    // the request. `db-shard-group` is a value the model's `SourceType` enum carries but
    // this operation's own documented valid values do not, and this front door publishes no
    // categories for it (see `state::EVENT_CATEGORIES`).
    for case in [
        "db-instances",
        "DB-INSTANCE",
        "db-shard-group",
        "not-a-source-type",
    ] {
        let body = expect_error(
            case,
            call(&router, "DescribeEventCategories", &[("SourceType", case)]).await,
            400,
            "InvalidParameterValue",
        );
        let message = text(&body, "Message");
        assert!(
            message.contains("db-instance"),
            "case {case:?}: expected the refusal to name the source types the front door \
             accepts, got {message:?}"
        );
        // The sentence is fixed, so it echoes the rejected value only where that value is
        // one of the names it lists anyway.
        assert!(
            ::service_common::rds_event_source_types::EVENT_SOURCE_TYPES.contains(&case)
                || !message.contains(case),
            "case {case:?}: expected the refusal not to echo the rejected value, got \
             {message:?}"
        );
    }

    // `Filters` is documented as "not currently supported" here too.
    expect_error(
        "a filter this operation does not support",
        call(
            &router,
            "DescribeEventCategories",
            &[
                ("Filters.Filter.1.Name", "source-type"),
                ("Filters.Filter.1.Values.Value.1", "db-instance"),
            ],
        )
        .await,
        400,
        "InvalidParameterValue",
    );
}

#[tokio::test]
async fn the_published_event_categories_are_the_documented_ones() {
    let router = emulator();

    // (source type, every event category AWS publishes for it, in the order the front
    // door's own report publishes them — the published "Amazon RDS event categories and
    // event messages" table's order for the entries no report of the service is evidenced
    // for).
    //
    // This table states the catalog INDEPENDENTLY of `state::EVENT_CATEGORIES`, which is
    // what the sibling test above compares the answer against: a row here is what the
    // published report says, so a change to the catalog that drops, invents or re-sorts a
    // category fails here rather than agreeing with itself. Every published source type is
    // a row, including the ones no issue cited (`db-cluster`, `db-parameter-group`,
    // `db-security-group`, `db-proxy`, `custom-engine-version`).
    let rows: Vec<(&str, Vec<&str>)> = vec![
        (
            "db-instance",
            vec![
                "backup",
                "deletion",
                "availability",
                "creation",
                "low storage",
                "restoration",
                "configuration change",
                "failover",
                "maintenance",
                "failure",
                "notification",
                "read replica",
                "recovery",
                "security",
                "backtrack",
                "security patching",
            ],
        ),
        (
            "db-cluster",
            vec![
                "failover",
                // The two categories only a CLUSTER publishes: its migration onto the
                // service, and its serverless capacity.
                "migration",
                "failure",
                "notification",
                "serverless",
                "creation",
                "deletion",
                "maintenance",
                "configuration change",
                "global-failover",
            ],
        ),
        ("db-parameter-group", vec!["configuration change"]),
        ("db-security-group", vec!["configuration change", "failure"]),
        (
            "db-snapshot",
            vec!["notification", "creation", "deletion", "restoration"],
        ),
        ("db-cluster-snapshot", vec!["backup", "notification"]),
        // A source type the model's `SourceType` enum declares and this operation's own
        // prose does not: its published event table has one row, a failed creation.
        ("custom-engine-version", vec!["failure"]),
        (
            "db-proxy",
            vec!["creation", "configuration change", "deletion", "failure"],
        ),
    ];

    // The whole catalog, so each row is checked both narrowed to its source type and as
    // the map the unfiltered report carries.
    let whole = expect_ok(
        "every published source type",
        call(&router, "DescribeEventCategories", &NO_SOURCE_TYPE).await,
    );
    let maps: Vec<String> = subtrees(&whole, "EventCategoriesMap");
    assert_eq!(
        maps.len(),
        rows.len(),
        "the unfiltered report carries one map per published source type, got {whole}"
    );

    for (source_type, expected) in rows {
        let narrowed = expect_ok(
            source_type,
            call(
                &router,
                "DescribeEventCategories",
                &[("SourceType", source_type)],
            )
            .await,
        );
        let reported = texts(&narrowed, "EventCategory");
        assert_eq!(
            reported, expected,
            "case {source_type:?}: expected the published categories {expected:?}, got \
             {reported:?} in {narrowed}"
        );
        let from_whole = maps
            .iter()
            .find(|map| text(map, "SourceType") == source_type)
            .unwrap_or_else(|| {
                panic!("case {source_type:?}: the unfiltered report must carry its map: {whole}")
            });
        assert_eq!(
            texts(from_whole, "EventCategory"),
            expected,
            "case {source_type:?}: the unfiltered report must carry the same categories as \
             the narrowed one, got {from_whole}"
        );
    }
}

#[tokio::test]
async fn the_other_control_planes_this_front_door_serves_report_their_own_engines() {
    let router = emulator();

    // The DocumentDB and Neptune control planes are served by THIS endpoint (see
    // `src/service.rs`), so its two engine reports carry their engines as well — which is
    // what `aws docdb describe-db-engine-versions` and
    // `aws docdb describe-orderable-db-instance-options` read. Neither engine is a value
    // this model's own `Engine` member lists, which is exactly why they are a row here.
    // (engine, the one licence model its orderable options are offered under). The
    // licence setting "doesn't apply to" the document engine, whose options carry `na`;
    // the graph engine is licensed by AWS itself and carries `amazon-license`, so the
    // licence is read per engine rather than assumed to be the same for both.
    for (engine, expected_license) in [("docdb", "na"), ("neptune", "amazon-license")] {
        let body = expect_ok(
            engine,
            call(
                &router,
                "DescribeDBEngineVersions",
                &[("Engine", engine), ("IncludeAll", "true")],
            )
            .await,
        );
        let reported = subtrees(&body, "DBEngineVersion");
        assert!(
            !reported.is_empty(),
            "case {engine:?}: this front door serves that control plane, so it must report \
             at least one of its engine versions, got {body}"
        );
        let mut versions: Vec<String> = Vec::new();
        for version in &reported {
            // An upgrade target carries its own `<Engine>`, so read the members off the
            // entry with its `<ValidUpgradeTarget>` subtrees removed.
            let mut scalars = version.clone();
            for target in subtrees(version, "ValidUpgradeTarget") {
                scalars = scalars.replace(&target, "");
            }
            assert_eq!(
                text(&scalars, "Engine"),
                engine,
                "case {engine:?}: Engine narrows the report to that engine, got {scalars}"
            );
            let family = text(&scalars, "DBParameterGroupFamily");
            assert!(
                family.starts_with(engine),
                "case {engine:?}: DBParameterGroupFamily {family:?} must be a family of \
                 this engine, got {scalars}"
            );
            versions.push(text(&scalars, "EngineVersion"));
        }

        let body = expect_ok(
            engine,
            call(
                &router,
                "DescribeOrderableDBInstanceOptions",
                &[("Engine", engine)],
            )
            .await,
        );
        let options = subtrees(&body, "OrderableDBInstanceOption");
        assert!(
            !options.is_empty(),
            "case {engine:?}: an engine whose versions this front door reports must have \
             orderable options, got {body}"
        );
        for option in &options {
            assert_eq!(
                text(option, "Engine"),
                engine,
                "case {engine:?}: every option reported must be of the engine asked for, \
                 got {option}"
            );
            let version = text(option, "EngineVersion");
            assert!(
                versions.contains(&version),
                "case {engine:?}: an option is offered for a version this service reports, \
                 got {version:?} which is not one of {versions:?}"
            );
            assert_eq!(
                text(option, "LicenseModel"),
                expected_license,
                "case {engine:?}: expected every option to be offered under \
                 {expected_license:?}, got {option}"
            );
        }
    }
}

#[tokio::test]
async fn default_only_with_no_resolvable_default_engine_version_is_refused() {
    let router = emulator();

    // `DefaultOnly` reports "only the default version of the specified engine or the
    // engine and major version combination", so there has to BE a specified engine with a
    // published default. (An `Engine` that names no engine at all is refused before this
    // gate, as the value its member does not accept — see
    // `tests/all/refusal_wording_and_input_guards.rs`.) (case, parameters)
    let refusals: Vec<RefusalRow> = vec![
        ("DefaultOnly with no engine at all", vec![("DefaultOnly", "true")]),
        (
            "DefaultOnly with a MaxRecords but still no engine",
            vec![("DefaultOnly", "true"), ("MaxRecords", "20")],
        ),
        (
            "DefaultOnly for an engine this front door publishes no version for",
            vec![("DefaultOnly", "true"), ("Engine", "custom-oracle-ee")],
        ),
        (
            "DefaultOnly for a parameter group family no engine publishes",
            vec![
                ("DefaultOnly", "true"),
                ("DBParameterGroupFamily", "docdb9.9"),
            ],
        ),
        // An engine this front door reports on behalf of ANOTHER control plane publishes
        // its versions here, and the DEFAULT among them wherever that control plane
        // publishes one: the graph engine does, and the document engine's own recorded
        // behaviour is the refusal below.
        (
            "DefaultOnly for the Amazon DocumentDB engine",
            vec![("DefaultOnly", "true"), ("Engine", "docdb")],
        ),
        (
            "DefaultOnly narrowed to one DocumentDB major version",
            vec![
                ("DefaultOnly", "true"),
                ("Engine", "docdb"),
                ("DBParameterGroupFamily", "docdb5.0"),
            ],
        ),
    ];

    for (case, params) in refusals {
        let body = expect_error(
            case,
            call(&router, "DescribeDBEngineVersions", &params).await,
            400,
            "InvalidParameterCombination",
        );
        let message = text(&body, "Message");
        assert!(
            message.contains("default engine version"),
            "case {case:?}: expected the refusal to say no default engine version could be \
             found, got {message:?}"
        );
    }

    // The same request with an engine that HAS a published default is served, so the
    // refusal above is about the missing default and not about `DefaultOnly` itself.
    for (case, params) in [
        (
            "DefaultOnly for an engine with a published default",
            vec![("DefaultOnly", "true"), ("Engine", "postgres")],
        ),
        (
            "DefaultOnly with the engine named by a filter",
            vec![
                ("DefaultOnly", "true"),
                ("Filters.Filter.1.Name", "engine"),
                ("Filters.Filter.1.Values.Value.1", "mysql"),
            ],
        ),
        // The graph engine of the Amazon Neptune control plane this door serves publishes a
        // default release of its own, so `DefaultOnly` reports it rather than refusing.
        (
            "DefaultOnly for the Amazon Neptune engine",
            vec![("DefaultOnly", "true"), ("Engine", "neptune")],
        ),
    ] {
        let body = expect_ok(case, call(&router, "DescribeDBEngineVersions", &params).await);
        let reported = subtrees(&body, "DBEngineVersion");
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected exactly the engine's default version, got {body}"
        );
    }
}

#[tokio::test]
async fn an_api_version_this_front_door_does_not_serve_is_refused_as_invalid_action() {
    let router = emulator();

    // (case, action, the Version the envelope names, the expected status, the expected
    // wire `<Code>`)
    //
    // The served rows use `DescribeEventCategories`, which this front door answers 200;
    // the refused rows span an older API version, a future one and a value that is not a
    // date, over three different actions, so what they pin is the envelope's version
    // check rather than any one handler.
    let rows: [(&str, &str, Option<&str>, u16, &str); 5] = [
        (
            "the version this model declares",
            "DescribeEventCategories",
            Some(API_VERSION),
            200,
            "",
        ),
        (
            "no version at all: the action alone decides",
            "DescribeEventCategories",
            None,
            200,
            "",
        ),
        (
            "a future version",
            "DescribeDBClusters",
            Some("2099-01-01"),
            400,
            "InvalidAction",
        ),
        (
            "an older version of this API",
            "DescribeDBEngineVersions",
            Some("2013-01-10"),
            400,
            "InvalidAction",
        ),
        (
            "a version that is not a date at all",
            "DescribeEventCategories",
            Some("latest"),
            400,
            "InvalidAction",
        ),
    ];

    for (case, action, version, expected_status, expected_code) in rows {
        let (status, content_type, body) = call_versioned(&router, action, version).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for {action} at version \
             {version:?}, got {status} with {body}"
        );
        assert!(
            content_type.starts_with("text/xml") || content_type.starts_with("application/xml"),
            "case {case:?}: an awsQuery answer is XML, got content-type {content_type:?} with \
             {body}"
        );
        if expected_code.is_empty() {
            assert!(
                body.contains(&format!("<{action}Response")),
                "case {case:?}: expected the {action} response envelope, got {body}"
            );
            continue;
        }
        assert_eq!(
            text(&body, "Code"),
            expected_code,
            "case {case:?}: expected the error code {expected_code}, got {body}"
        );
        assert_eq!(
            text(&body, "Type"),
            "Sender",
            "case {case:?}: a bad request is the sender's fault: {body}"
        );
        let message = text(&body, "Message");
        let version = version.unwrap_or_default();
        assert!(
            message.contains(action) && message.contains(version),
            "case {case:?}: expected the refusal to name the action and the version it was \
             asked for, got {message:?}"
        );
    }
}
