// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's DB instance slice: `CreateDBInstance`,
//! `DescribeDBInstances`, `ModifyDBInstance`, `CreateDBSnapshot`,
//! `DescribeDBSnapshots` and `DeleteDBInstance`.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter`
//! exactly as the production router mounts it, and each request is a real awsQuery
//! form POST carrying the SigV4 credential scope the router keys on. The assertions
//! read the XML the emulator answers with, so a handler that computed nothing could
//! not pass them.
//!
//! The tables below span the classes the rules generalise over — the engine families
//! the default port is drawn from, the four branches of the documented
//! `PubliclyAccessible` default, each published bound of `Port`,
//! `BackupRetentionPeriod` and `MaxRecords` plus one value past it, and the identifier
//! shapes the `DBInstanceIdentifier` constraints admit and refuse.
//!
//! The wire harness here — the router, the `awsQuery` call, the status/`<Code>`
//! assertions and the XML element readers — is `pub(crate)`, so the crate's other
//! suites drive this service over the wire the same way rather than growing a second
//! copy of it.

use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

use service_com_amazonaws_rds_amazonrdsv19::state;
use service_com_amazonaws_rds_amazonrdsv19::state::{
    DB_INSTANCE_QUOTA_PER_REGION, DB_SNAPSHOT_QUOTA_PER_REGION, MAX_FILTER_TERMS, MAX_REGIONS,
    STATUS_TRANSITION_DELAY,
};
use service_com_amazonaws_rds_amazonrdsv19::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for, unless it names another.
pub(crate) const REGION: &str = "us-west-2";

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// How long a test waits before expecting a transition this service owes to have
/// settled: the span itself plus slack for a loaded test machine.
///
/// `pub(crate)` so the sibling suites of this one test target wait out the same span.
pub(crate) fn wait_for_settle() {
    sleep(STATUS_TRANSITION_DELAY + Duration::from_millis(200));
}

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// The query parameters of one call, owned so a table row can hold them.
type Params = Vec<(&'static str, String)>;

/// A call that carries no parameters beyond `Action` and `Version`.
const NO_PARAMS: [(&str, &str); 0] = [];

/// A router carrying only this service over one shared engine — what the production
/// router does for every service at once.
///
/// `pub(crate)` so the sibling suites of this one test target drive the service through
/// the same front door rather than standing up their own.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Verifying signatures is the emulator's concern, not this service's; these tests
    // sign nothing, so the check is off and the scope header is read for routing only.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// One awsQuery call against `region`, returning the HTTP status and the XML body.
pub(crate) async fn call_in<N: AsRef<str>, V: AsRef<str>>(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[(N, V)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2014-10-31");
    for (name, value) in params {
        let (name, value) = (name.as_ref(), value.as_ref());
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
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

/// One form value as a client puts it in an `application/x-www-form-urlencoded` body:
/// every byte outside the unreserved set (`A-Z`, `a-z`, `0-9`, `-`, `_`, `.`, `~`) is
/// percent-escaped.
///
/// Escaping rather than refusing is what lets a row send a value whose own alphabet
/// overlaps the body's syntax — an opaque pagination token is base64, so `+`, `/` and `=`
/// are in it — exactly as an SDK would send it.
fn form_encode(value: &str) -> String {
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

/// One awsQuery call against [`REGION`].
pub(crate) async fn call<N: AsRef<str>, V: AsRef<str>>(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(N, V)],
) -> (u16, String) {
    call_in(router, REGION, action, params).await
}

/// `CreateDBInstance` with its three required members plus `extra`.
async fn create_instance(
    router: &MultiServiceRouter,
    identifier: &str,
    engine: &str,
    class: &str,
    extra: &[(&str, String)],
) -> (u16, String) {
    let mut params: Vec<(&str, String)> = params![
        "DBInstanceIdentifier" => identifier,
        "Engine" => engine,
        "DBInstanceClass" => class,
    ]
    .to_vec();
    params.extend(extra.iter().cloned());
    call(router, "CreateDBInstance", &params).await
}

/// Create an instance and wait until it has reached `available`.
async fn create_available_instance(
    router: &MultiServiceRouter,
    identifier: &str,
    engine: &str,
    class: &str,
    extra: &[(&str, String)],
) {
    let case = format!("create {identifier}");
    expect_ok(&case, create_instance(router, identifier, engine, class, extra).await);
    wait_for_settle();
}

/// Assert a 200 and hand back the body.
pub(crate) fn expect_ok(case: &str, response: (u16, String)) -> String {
    let (status, body) = response;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

/// Assert the modelled failure: its HTTP status and its wire error `<Code>`.
pub(crate) fn expect_error(
    case: &str,
    response: (u16, String),
    expected_status: u16,
    expected_code: &str,
) -> String {
    let (status, body) = response;
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
    );
    let code = text(&body, "Code");
    assert_eq!(
        code, expected_code,
        "case {case:?}: expected error code {expected_code}, got {code:?} in {body}"
    );
    body
}

/// Where the next `<tag …>` after `from` starts, where its content starts, and
/// whether it closed itself.
///
/// These helpers are NOT an XML parser and are not meant to be one: they read back
/// the documents this emulator itself produced — machine-generated element trees with
/// no comments, no CDATA, no processing instructions and no same-name nesting (which
/// [`element_spans`] asserts) — so locating a start tag and its matching end tag is
/// all a member read needs. Parsing arbitrary XML is a crate's job; reading one
/// member out of a response this test just received is not, and keeping it
/// dependency-free leaves `Cargo.lock` untouched by this change.
fn next_open_tag(xml: &str, tag: &str, from: usize) -> Option<(usize, usize, bool)> {
    let needle = format!("<{tag}");
    let mut cursor = from;
    while let Some(offset) = xml[cursor..].find(&needle) {
        let start = cursor + offset;
        let after = start + needle.len();
        // `<DBInstance>` must not match `<DBInstanceArn>`: only `>`, `/` or a space
        // can follow an element name.
        let next = xml[after..].chars().next()?;
        if next == '>' || next == '/' || next.is_whitespace() {
            let end = after + xml[after..].find('>')?;
            return Some((start, end + 1, xml[..end].ends_with('/')));
        }
        cursor = after;
    }
    None
}

/// Every `<tag>` element in `xml`, in document order, as
/// `(element start, content start, content end, element end)` byte offsets.
fn element_spans(xml: &str, tag: &str) -> Vec<(usize, usize, usize, usize)> {
    let close = format!("</{tag}>");
    let mut found = Vec::new();
    let mut cursor = 0usize;
    while let Some((start, content_start, self_closing)) = next_open_tag(xml, tag, cursor) {
        if self_closing {
            found.push((start, content_start, content_start, content_start));
            cursor = content_start;
            continue;
        }
        let offset = xml[content_start..].find(&close).unwrap_or_else(|| {
            panic!("the emulator's response must close every <{tag}> it opens: {xml}")
        });
        let content_end = content_start + offset;
        assert!(
            next_open_tag(xml, tag, content_start).is_none_or(|(next, _, _)| next >= content_end),
            "<{tag}> nests inside itself, which these helpers do not read: {xml}"
        );
        let end = content_end + close.len();
        found.push((start, content_start, content_end, end));
        cursor = end;
    }
    found
}

/// The five entities the XML specification predefines, resolved.
fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        // `&amp;` last, so `&amp;lt;` resolves to the TEXT `&lt;` and not to `<`.
        .replace("&amp;", "&")
}

/// The text of every `<tag>` element in `xml`, in document order.
pub(crate) fn texts(xml: &str, tag: &str) -> Vec<String> {
    element_spans(xml, tag)
        .into_iter()
        .map(|(_, content_start, content_end, _)| unescape(&xml[content_start..content_end]))
        .collect()
}

/// How many elements enclose the one that starts at `offset`.
///
/// A member read below reads the member of the RESOURCE a response carries, and a response
/// can carry an element of the same name deeper inside it — a `DBCluster` reports its own
/// `Status` and carries another `Status` inside each of its security-group and option-group
/// memberships — so "the one `<tag>`" is only unambiguous at one depth.
fn depth_at(xml: &str, offset: usize) -> usize {
    let mut depth = 0usize;
    let mut cursor = 0usize;
    while let Some(found) = xml[cursor..offset].find('<') {
        let start = cursor + found;
        let Some(close) = xml[start..].find('>') else { break };
        let tag = &xml[start + 1..start + close];
        cursor = start + close + 1;
        if tag.starts_with('?') || tag.starts_with('!') || tag.ends_with('/') {
            continue;
        }
        if tag.starts_with('/') {
            depth = depth.saturating_sub(1);
        } else {
            depth += 1;
        }
    }
    depth
}

/// The text of every `<tag>` element that sits at the SHALLOWEST depth any `<tag>` in `xml`
/// sits at: the ones that are members of the resource `xml` carries rather than of
/// something nested inside it (see [`depth_at`]).
fn shallowest_texts(xml: &str, tag: &str) -> Vec<String> {
    let spans = element_spans(xml, tag);
    let Some(shallowest) = spans
        .iter()
        .map(|(start, ..)| depth_at(xml, *start))
        .min()
    else {
        return Vec::new();
    };
    spans
        .into_iter()
        .filter(|(start, ..)| depth_at(xml, *start) == shallowest)
        .map(|(_, content_start, content_end, _)| unescape(&xml[content_start..content_end]))
        .collect()
}

/// The text of the one `<tag>` element `xml` carries as a member of its own resource; see
/// [`shallowest_texts`].
pub(crate) fn text(xml: &str, tag: &str) -> String {
    let found = shallowest_texts(xml, tag);
    assert_eq!(
        found.len(),
        1,
        "expected exactly one <{tag}> in {xml}, found {}",
        found.len()
    );
    found.into_iter().next().expect("checked just above")
}

/// The text of the one `<tag>` element `xml` carries as a member of its own resource, or
/// `None` when it carries none; see [`shallowest_texts`].
pub(crate) fn optional_text(xml: &str, tag: &str) -> Option<String> {
    let found = shallowest_texts(xml, tag);
    assert!(
        found.len() <= 1,
        "expected at most one <{tag}> in {xml}, found {}",
        found.len()
    );
    found.into_iter().next()
}

/// The XML of each `<tag>` subtree, in document order.
pub(crate) fn subtrees(xml: &str, tag: &str) -> Vec<String> {
    element_spans(xml, tag)
        .into_iter()
        .map(|(start, _, _, end)| xml[start..end.max(start)].to_string())
        .collect()
}

/// The one `<DBInstance>` subtree in `xml`.
fn the_instance(case: &str, xml: &str) -> String {
    let found = subtrees(xml, "DBInstance");
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one DBInstance, got {} in {xml}",
        found.len()
    );
    found.into_iter().next().expect("checked just above")
}

/// `DescribeDBInstances` for one identifier, as the one `<DBInstance>` it reports.
async fn describe_one(router: &MultiServiceRouter, case: &str, identifier: &str) -> String {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeDBInstances",
            &params!["DBInstanceIdentifier" => identifier],
        )
        .await,
    );
    the_instance(case, &body)
}

/// The identifiers `DescribeDBInstances` reports, in the order it reports them.
async fn list_instance_identifiers(
    router: &MultiServiceRouter,
    extra: &[(&str, String)],
) -> (Vec<String>, Option<String>) {
    let body = expect_ok(
        "list instances",
        call(router, "DescribeDBInstances", extra).await,
    );
    let identifiers = subtrees(&body, "DBInstance")
        .iter()
        .map(|instance| text(instance, "DBInstanceIdentifier"))
        .collect();
    (identifiers, optional_text(&body, "Marker"))
}

// ---------------------------------------------------------------------------
// CreateDBInstance / DescribeDBInstances
// ---------------------------------------------------------------------------

/// One row of the create/round-trip table.
struct InstanceCase {
    case: &'static str,
    identifier: &'static str,
    engine: &'static str,
    class: &'static str,
    extra: &'static [(&'static str, &'static str)],
    /// The port the request asks for, or the documented per-engine default.
    expected_port: &'static str,
    /// The documented `PubliclyAccessible` default for this engine and subnet group.
    expected_publicly_accessible: &'static str,
}

/// The table spans the engine families `CreateDBInstanceMessage$Port` lists a default
/// for — including Db2 and MariaDB, which the issue never cited — and all four
/// branches of the documented `PubliclyAccessible` default.
const INSTANCE_CASES: &[InstanceCase] = &[
    InstanceCase {
        case: "postgres, every optional supplied",
        identifier: "emu-pg",
        engine: "postgres",
        class: "db.t3.micro",
        extra: &[
            ("AllocatedStorage", "20"),
            ("MasterUsername", "pgadmin"),
            ("MasterUserPassword", "secret99"),
            ("BackupRetentionPeriod", "7"),
            ("DBName", "appdb"),
            ("EngineVersion", "16.4"),
            ("StorageEncrypted", "true"),
            ("StorageType", "gp3"),
            ("Tags.Tag.1.Key", "team"),
            ("Tags.Tag.1.Value", "platform"),
        ],
        expected_port: "5432",
        expected_publicly_accessible: "true",
    },
    InstanceCase {
        case: "mysql, explicit port and Multi-AZ",
        identifier: "emu-my",
        engine: "mysql",
        class: "db.m5.large",
        extra: &[
            ("AllocatedStorage", "100"),
            ("MasterUsername", "root"),
            ("Port", "13306"),
            ("MultiAZ", "true"),
            ("PubliclyAccessible", "false"),
            ("DeletionProtection", "true"),
            ("BackupRetentionPeriod", "0"),
        ],
        expected_port: "13306",
        expected_publicly_accessible: "false",
    },
    InstanceCase {
        case: "oracle takes the Oracle default port",
        identifier: "emu-ora",
        engine: "oracle-se2",
        class: "db.m5.xlarge",
        extra: &[("AllocatedStorage", "40")],
        expected_port: "1521",
        expected_publicly_accessible: "true",
    },
    InstanceCase {
        case: "sql server takes the SQL Server default port",
        identifier: "emu-mssql",
        engine: "sqlserver-ex",
        class: "db.t3.small",
        extra: &[("AllocatedStorage", "30")],
        expected_port: "1433",
        expected_publicly_accessible: "true",
    },
    InstanceCase {
        case: "mariadb takes the MariaDB default port",
        identifier: "emu-maria",
        engine: "mariadb",
        class: "db.t3.medium",
        extra: &[("AllocatedStorage", "25")],
        expected_port: "3306",
        expected_publicly_accessible: "true",
    },
    InstanceCase {
        case: "db2 takes the Db2 default port",
        identifier: "emu-db2",
        engine: "db2-se",
        class: "db.m6i.large",
        extra: &[("AllocatedStorage", "60")],
        expected_port: "50000",
        expected_publicly_accessible: "true",
    },
    InstanceCase {
        case: "aurora without a subnet group is not publicly accessible",
        identifier: "emu-aurora-pg",
        engine: "aurora-postgresql",
        class: "db.r6g.large",
        extra: &[],
        expected_port: "5432",
        expected_publicly_accessible: "false",
    },
    InstanceCase {
        case: "the default subnet group makes it publicly accessible",
        identifier: "emu-aurora-my",
        engine: "aurora-mysql",
        class: "db.r6g.large",
        extra: &[("DBSubnetGroupName", "default")],
        expected_port: "3306",
        expected_publicly_accessible: "true",
    },
    InstanceCase {
        case: "a named subnet group is not publicly accessible",
        identifier: "emu-custom-mssql",
        engine: "custom-sqlserver-ee",
        class: "db.m5.large",
        extra: &[("DBSubnetGroupName", "emu-subnets")],
        expected_port: "1433",
        expected_publicly_accessible: "false",
    },
];

#[tokio::test]
async fn create_persists_every_setting_and_describe_reads_it_back() {
    let router = emulator();
    let mut hosted_zone_ids = Vec::new();

    // One row names a DB subnet group of its own, and a `DBSubnetGroupName` is now
    // RESOLVED against the store rather than echoed, so the group has to exist before
    // an instance can reference it.
    expect_ok(
        "the named subnet group is created first",
        call(
            &router,
            "CreateDBSubnetGroup",
            &params![
                "DBSubnetGroupName" => "emu-subnets",
                "DBSubnetGroupDescription" => "two subnets",
                "SubnetIds.SubnetIdentifier.1" => "subnet-0a1b2c3d",
                "SubnetIds.SubnetIdentifier.2" => "subnet-1b2c3d4e",
            ],
        )
        .await,
    );

    for row in INSTANCE_CASES {
        let extra: Vec<(&str, String)> = row
            .extra
            .iter()
            .map(|(name, value)| (*name, (*value).to_string()))
            .collect();
        let created = expect_ok(
            row.case,
            create_instance(&router, row.identifier, row.engine, row.class, &extra).await,
        );
        let created = the_instance(row.case, &created);

        // A freshly created DB instance is not usable yet.
        let status = text(&created, "DBInstanceStatus");
        assert_eq!(
            status, "creating",
            "case {:?}: expected DBInstanceStatus creating on create, got {status:?}",
            row.case
        );

        // The same record, read back out of the store through DescribeDBInstances.
        let read_back = describe_one(&router, row.case, row.identifier).await;

        for (source, where_from) in [(&created, "CreateDBInstance"), (&read_back, "Describe")] {
            let identifier = text(source, "DBInstanceIdentifier");
            assert_eq!(
                identifier, row.identifier,
                "case {:?}: {where_from} reported identifier {identifier:?}",
                row.case
            );
            let engine = text(source, "Engine");
            assert_eq!(
                engine, row.engine,
                "case {:?}: {where_from} reported engine {engine:?}",
                row.case
            );
            let class = text(source, "DBInstanceClass");
            assert_eq!(
                class, row.class,
                "case {:?}: {where_from} reported class {class:?}",
                row.case
            );
            let port = text(source, "Port");
            // The port the instance PUBLISHES is the port a client connects to. For an
            // engine whose wire this emulator serves, that is the port the core assigned
            // its own engine listener (`state::data_plane`), which is what makes the
            // published endpoint reachable — that it ANSWERS there is asserted in
            // `db_instance_engine_endpoint`; for every other engine it is the port the
            // request asked for or the engine's documented default.
            if state::engine_wire(row.engine).is_some() {
                assert!(
                    port.parse::<u16>().is_ok_and(|port| port > 0),
                    "case {:?}: {where_from} reported endpoint port {port:?}, expected the \
                     port the instance's engine listener is bound to",
                    row.case
                );
            } else {
                assert_eq!(
                    port, row.expected_port,
                    "case {:?}: {where_from} reported endpoint port {port:?}",
                    row.case
                );
            }
            let publicly_accessible = text(source, "PubliclyAccessible");
            assert_eq!(
                publicly_accessible, row.expected_publicly_accessible,
                "case {:?}: {where_from} reported PubliclyAccessible {publicly_accessible:?}",
                row.case
            );
            let arn = text(source, "DBInstanceArn");
            assert_eq!(
                arn,
                format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:{}", row.identifier),
                "case {:?}: {where_from} reported ARN {arn:?}",
                row.case
            );
            let resource_id = text(source, "DbiResourceId");
            assert!(
                resource_id.starts_with("db-") && resource_id.len() == 29,
                "case {:?}: {where_from} reported DbiResourceId {resource_id:?}, expected \
                 'db-' and 26 more characters",
                row.case
            );
            let address = text(source, "Address");
            // The address the instance PUBLISHES is the address a client connects to:
            // the host its own engine listener is bound to for an engine this emulator
            // serves — which `db_instance_engine_endpoint` opens a real session on — and
            // the DNS name of its endpoint for one it does not.
            if state::engine_wire(row.engine).is_some() {
                assert!(
                    address
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|host| host.is_loopback()),
                    "case {:?}: {where_from} reported endpoint address {address:?}, \
                     expected the host of the instance's engine listener",
                    row.case
                );
            } else {
                assert!(
                    address.starts_with(&format!("{}.", row.identifier))
                        && address.ends_with(&format!(".{REGION}.rds.amazonaws.com")),
                    "case {:?}: {where_from} reported endpoint address {address:?}",
                    row.case
                );
            }
            assert!(
                !text(source, "InstanceCreateTime").is_empty(),
                "case {:?}: {where_from} reported no InstanceCreateTime",
                row.case
            );
            let availability_zone = text(source, "AvailabilityZone");
            assert!(
                availability_zone.starts_with(REGION),
                "case {:?}: {where_from} reported AvailabilityZone {availability_zone:?}",
                row.case
            );

            // Every optional the request supplied comes back from the store.
            for (name, expected) in row.extra {
                let member = match *name {
                    // The password is `@sensitive`: RDS reports no member for it, so
                    // it must appear nowhere in the response.
                    "MasterUserPassword" => {
                        assert!(
                            !source.contains(expected),
                            "case {:?}: {where_from} leaked the master user password",
                            row.case
                        );
                        continue;
                    }
                    // Checked above through the rules they feed.
                    "Port" | "PubliclyAccessible" => continue,
                    "Tags.Tag.1.Key" => "Key",
                    "Tags.Tag.1.Value" => "Value",
                    "DBSubnetGroupName" => "DBSubnetGroupName",
                    other => other,
                };
                let actual = optional_text(source, member);
                assert_eq!(
                    actual.as_deref(),
                    Some(*expected),
                    "case {:?}: {where_from} reported {member} as {actual:?}, expected \
                     the value the request supplied",
                    row.case
                );
            }
        }

        hosted_zone_ids.push(optional_text(&read_back, "HostedZoneId"));
    }

    // Every row ran in the same region and under the same control plane's endpoint domain,
    // so every row reports the SAME hosted zone id: there is one Route 53 zone per region
    // and domain, which is the invariance the member promises.
    let expected_zone = state::hosted_zone_id(REGION, state::RDS_ENDPOINT_DOMAIN);
    for (row, hosted_zone_id) in INSTANCE_CASES.iter().zip(&hosted_zone_ids) {
        assert_eq!(
            hosted_zone_id.as_deref(),
            Some(expected_zone.as_str()),
            "case {:?}: expected the one hosted zone id of {REGION}, got {hosted_zone_id:?}",
            row.case
        );
    }

    // The unfiltered report carries every stored instance, ordered by identifier.
    let (identifiers, marker) = list_instance_identifiers(&router, &[]).await;
    let mut expected: Vec<&str> = INSTANCE_CASES.iter().map(|row| row.identifier).collect();
    expected.sort_unstable();
    assert_eq!(
        identifiers, expected,
        "the unfiltered report must list every stored instance in identifier order"
    );
    assert_eq!(
        marker, None,
        "no Marker is due when every record fits in one page, got {marker:?}"
    );
}

#[tokio::test]
async fn one_identifier_names_a_different_instance_in_each_region() {
    let router = emulator();
    // RDS is regional: the SAME identifier is a different instance in each region, and
    // creating it in a second region is not a collision.
    let regions = ["us-west-2", "eu-central-1", "us-east-1"];
    for region in regions {
        expect_ok(
            region,
            call_in(
                &router,
                region,
                "CreateDBInstance",
                &params![
                    "DBInstanceIdentifier" => "emu-regional",
                    "Engine" => "postgres",
                    "DBInstanceClass" => "db.t3.micro",
                ],
            )
            .await,
        );
    }

    let mut hosted_zones = Vec::new();
    let mut resource_ids = Vec::new();
    for region in regions {
        let body = expect_ok(
            region,
            call_in(
                &router,
                region,
                "DescribeDBInstances",
                &params!["DBInstanceIdentifier" => "emu-regional"],
            )
            .await,
        );
        // Exactly one instance per region, never the other regions' records.
        let instance = the_instance(region, &body);
        assert_eq!(
            text(&instance, "DBInstanceArn"),
            format!("arn:aws:rds:{region}:{ACCOUNT}:db:emu-regional"),
            "case {region:?}: the ARN must carry the region the request was signed for"
        );
        assert!(
            text(&instance, "Address")
                .parse::<std::net::IpAddr>()
                .is_ok_and(|host| host.is_loopback()),
            "case {region:?}: a postgres instance publishes the host of its own engine \
             listener, got {:?}",
            text(&instance, "Address")
        );
        // The unfiltered report of this region holds only this region's one instance.
        let listed = expect_ok(
            region,
            call_in(&router, region, "DescribeDBInstances", &NO_PARAMS).await,
        );
        assert_eq!(
            subtrees(&listed, "DBInstance").len(),
            1,
            "case {region:?}: a describe must report only its own region's instances"
        );
        hosted_zones.push(optional_text(&instance, "HostedZoneId"));
        resource_ids.push(text(&instance, "DbiResourceId"));
    }

    for (index, region) in regions.iter().enumerate() {
        for (other_index, other) in regions.iter().enumerate() {
            if index == other_index {
                continue;
            }
            assert_ne!(
                resource_ids[index], resource_ids[other_index],
                "case {region:?} vs {other:?}: each instance gets its own resource id"
            );
        }
    }
    // The emulator reports the id AWS published where it can cite one, and a minted id of
    // the published form everywhere else — one per region, never absent, since every
    // endpoint it reports sits in some hosted zone.
    assert_eq!(
        hosted_zones[2].as_deref(),
        Some("Z2R2ITUGPM61AM"),
        "us-east-1 must report its published hosted zone id"
    );
    for index in [0usize, 1] {
        let expected = state::hosted_zone_id(regions[index], state::RDS_ENDPOINT_DOMAIN);
        assert_eq!(
            hosted_zones[index].as_deref(),
            Some(expected.as_str()),
            "case {:?}: a region must report its own hosted zone id, got {:?}",
            regions[index],
            hosted_zones[index]
        );
        assert_ne!(
            hosted_zones[index].as_deref(),
            hosted_zones[2].as_deref(),
            "case {:?}: two regions must not report one hosted zone",
            regions[index]
        );
    }
}

#[tokio::test]
async fn an_omitted_member_takes_its_documented_default() {
    let router = emulator();

    // `StorageType` "Default: io1, if the Iops parameter is specified. Otherwise,
    // gp3." — the two branches of the one documented rule.
    let storage_type_rows: &[(&str, &str, Params, &str)] = &[
        ("no Iops", "emu-default-gp3", Vec::new(), "gp3"),
        (
            "Iops supplied",
            "emu-default-io1",
            params!["Iops" => 3000].to_vec(),
            "io1",
        ),
    ];
    for (case, identifier, extra, expected) in storage_type_rows {
        expect_ok(
            case,
            create_instance(&router, identifier, "postgres", "db.t3.micro", extra).await,
        );
        let read_back = describe_one(&router, case, identifier).await;
        let actual = text(&read_back, "StorageType");
        assert_eq!(
            actual, *expected,
            "case {case:?}: expected the documented StorageType default {expected:?}, \
             got {actual:?}"
        );
    }

    // Every other documented default, read back out of the store rather than echoed:
    // `BackupRetentionPeriod` "Default: 1"; `StorageEncrypted` "By default, it isn't
    // encrypted"; `AutoMinorVersionUpgrade` "By default, minor engine upgrades are
    // applied automatically"; `CopyTagsToSnapshot` "By default, tags are not copied";
    // `DeletionProtection` "By default, deletion protection isn't enabled".
    let read_back = describe_one(&router, "no optional members at all", "emu-default-gp3").await;
    let defaults = [
        ("BackupRetentionPeriod", "1"),
        ("StorageEncrypted", "false"),
        ("AutoMinorVersionUpgrade", "true"),
        ("CopyTagsToSnapshot", "false"),
        ("DeletionProtection", "false"),
        ("MultiAZ", "false"),
    ];
    for (member, expected) in defaults {
        let actual = optional_text(&read_back, member);
        assert_eq!(
            actual.as_deref(),
            Some(expected),
            "case {member:?}: expected the documented default {expected:?}, got {actual:?}"
        );
    }
    // The defaults the service RESOLVES rather than reads off one member: the engine
    // version it provisioned (`postgres`'s default available catalogue version), the
    // default parameter and option groups named after that version, the backup and
    // maintenance windows it selected, and the CA its server certificate is issued
    // under. `db_instance_envelope` asserts the rules behind these over a table of
    // engines; here they pin that an instance created with nothing optional still
    // reports them.
    let resolved = [
        ("EngineVersion", "18.3"),
        ("DBParameterGroupName", "default.postgres18"),
        ("OptionGroupName", "default:postgres-18"),
        ("PreferredBackupWindow", "07:00-07:30"),
        ("PreferredMaintenanceWindow", "mon:08:00-mon:08:30"),
        ("CACertificateIdentifier", "rds-ca-rsa2048-g1"),
        ("LicenseModel", "postgresql-license"),
    ];
    for (member, expected) in resolved {
        let actual = optional_text(&read_back, member);
        assert_eq!(
            actual.as_deref(),
            Some(expected),
            "case {member:?}: expected the resolved default {expected:?}, got {actual:?}"
        );
    }
    // A member with no documented default and no supplied value is reported by no
    // member at all, rather than by an invented value.
    for member in ["AllocatedStorage", "MasterUsername", "DBName"] {
        assert_eq!(
            optional_text(&read_back, member),
            None,
            "case {member:?}: a member the request omitted and the model gives no default \
             for must be absent, got {:?}",
            optional_text(&read_back, member)
        );
    }
}

#[tokio::test]
async fn a_credential_scope_that_names_no_aws_region_is_refused() {
    let router = emulator();
    // The scope region keys the store, so only the published region-id form is taken:
    // lowercase letter groups then a one- or two-digit number.
    for region in ["us-east-1", "ap-southeast-4", "us-gov-west-1", "cn-north-1"] {
        expect_ok(
            region,
            call_in(
                &router,
                region,
                "CreateDBInstance",
                &params![
                    "DBInstanceIdentifier" => "emu-scope",
                    "Engine" => "postgres",
                    "DBInstanceClass" => "db.t3.micro",
                ],
            )
            .await,
        );
    }
    for region in [
        "useast1",
        "us-east-999",
        "US-EAST-1",
        "us--east-1",
        "this-region-name-is-far-too-long-to-be-one-1",
    ] {
        let body = expect_error(
            region,
            call_in(
                &router,
                region,
                "DescribeDBInstances",
                &NO_PARAMS,
            )
            .await,
            400,
            "ValidationException",
        );
        assert!(
            text(&body, "Message").contains("region"),
            "case {region:?}: the refusal must say the scope names no region, got {:?}",
            text(&body, "Message")
        );
    }
}

#[tokio::test]
async fn the_store_keeps_records_for_a_bounded_number_of_regions() {
    let router = emulator();
    // One instance in each of MAX_REGIONS regions fills the store's region capacity;
    // the next region is refused, while the regions already stored still have room.
    for index in 0..MAX_REGIONS {
        let region = emu_region(index);
        expect_ok(
            &region,
            call_in(
                &router,
                &region,
                "CreateDBInstance",
                &params![
                    "DBInstanceIdentifier" => "emu-bucket",
                    "Engine" => "postgres",
                    "DBInstanceClass" => "db.t3.micro",
                ],
            )
            .await,
        );
    }
    expect_error(
        "one region past the store's region capacity",
        call_in(
            &router,
            "ap-emuextra-1",
            "CreateDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-bucket",
                "Engine" => "postgres",
                "DBInstanceClass" => "db.t3.micro",
            ],
        )
        .await,
        400,
        "InstanceQuotaExceeded",
    );
    expect_ok(
        "a second instance in a region already stored",
        call_in(
            &router,
            &emu_region(0),
            "CreateDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-bucket-2",
                "Engine" => "postgres",
                "DBInstanceClass" => "db.t3.micro",
            ],
        )
        .await,
    );
}

/// The `index`th distinct region id this test uses: two lowercase letters, then
/// `-emu-1`, so it is of the published AWS region-id form and unique per index.
///
/// `pub(crate)` so the sibling suites that fill the store's region capacity use the
/// same generator rather than a second one of their own.
pub(crate) fn emu_region(index: usize) -> String {
    let first = (b'a' + (index / 8) as u8) as char;
    let second = (b'a' + (index % 8) as u8) as char;
    format!("{first}{second}-emu-1")
}

#[tokio::test]
async fn the_region_quotas_refuse_a_store_that_would_grow_past_them() {
    let router = emulator();
    // Filling the region's DB instance quota is allowed; one more is the modelled fault.
    for index in 0..DB_INSTANCE_QUOTA_PER_REGION {
        let identifier = format!("emu-quota-{index:03}");
        expect_ok(
            &identifier,
            create_instance(&router, &identifier, "postgres", "db.t3.micro", &[]).await,
        );
    }
    expect_error(
        "one DB instance past the region's quota",
        create_instance(&router, "emu-quota-over", "postgres", "db.t3.micro", &[]).await,
        400,
        "InstanceQuotaExceeded",
    );
    // The quota is per region, so another region still has its own room.
    expect_ok(
        "the same count in another region",
        call_in(
            &router,
            "eu-central-1",
            "CreateDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-quota-over",
                "Engine" => "postgres",
                "DBInstanceClass" => "db.t3.micro",
            ],
        )
        .await,
    );

    // The same for manual snapshots of one available instance.
    wait_for_settle();
    for index in 0..DB_SNAPSHOT_QUOTA_PER_REGION {
        let snapshot = format!("emu-quota-snap-{index:03}");
        expect_ok(
            &snapshot,
            call(
                &router,
                "CreateDBSnapshot",
                &params![
                    "DBInstanceIdentifier" => "emu-quota-000",
                    "DBSnapshotIdentifier" => snapshot,
                ],
            )
            .await,
        );
    }
    expect_error(
        "one snapshot past the region's quota",
        call(
            &router,
            "CreateDBSnapshot",
            &params![
                "DBInstanceIdentifier" => "emu-quota-000",
                "DBSnapshotIdentifier" => "emu-quota-snap-over",
            ],
        )
        .await,
        400,
        "SnapshotQuotaExceeded",
    );
}

#[tokio::test]
async fn a_describe_carrying_more_filter_values_than_the_emulator_serves_is_refused() {
    let router = emulator();
    create_available_instance(&router, "emu-terms", "postgres", "db.t3.micro", &[]).await;

    // At the bound the request is served; one value past it is refused.
    for (case, terms, served) in [
        ("at the filter-term bound", MAX_FILTER_TERMS, true),
        ("one past the filter-term bound", MAX_FILTER_TERMS + 1, false),
    ] {
        let mut request: Vec<(String, String)> =
            vec![("Filters.Filter.1.Name".to_string(), "engine".to_string())];
        for index in 1..=terms {
            request.push((
                format!("Filters.Filter.1.Values.Value.{index}"),
                format!("engine-{index}"),
            ));
        }
        let response = call(&router, "DescribeDBInstances", &request).await;
        if served {
            expect_ok(case, response);
        } else {
            let body = expect_error(case, response, 400, "ValidationException");
            assert!(
                text(&body, "Message").contains("Filters"),
                "case {case:?}: the refusal must name Filters, got {:?}",
                text(&body, "Message")
            );
        }
    }
}

#[tokio::test]
async fn the_identifier_is_folded_to_lowercase_and_matched_folded() {
    let router = emulator();
    let created = expect_ok(
        "mixed-case identifier",
        create_instance(&router, "Emu-Mixed-CASE", "postgres", "db.t3.micro", &[]).await,
    );
    let created = the_instance("mixed-case identifier", &created);
    let identifier = text(&created, "DBInstanceIdentifier");
    assert_eq!(
        identifier, "emu-mixed-case",
        "DBInstanceIdentifier is stored as a lowercase string, got {identifier:?}"
    );
    assert_eq!(
        text(&created, "DBInstanceArn"),
        format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:emu-mixed-case"),
        "the ARN must carry the folded identifier"
    );

    // Any casing finds the one stored record.
    for spelling in ["EMU-MIXED-CASE", "emu-mixed-case", "Emu-Mixed-Case"] {
        let read_back = describe_one(&router, spelling, spelling).await;
        assert_eq!(
            text(&read_back, "DBInstanceIdentifier"),
            "emu-mixed-case",
            "case {spelling:?}: a describe must fold the identifier the same way"
        );
    }

    // And a second create under any casing is the same instance.
    for spelling in ["emu-mixed-case", "EMU-Mixed-Case"] {
        expect_error(
            spelling,
            create_instance(&router, spelling, "postgres", "db.t3.micro", &[]).await,
            400,
            "DBInstanceAlreadyExists",
        );
    }
}

// ---------------------------------------------------------------------------
// Documented constraints
// ---------------------------------------------------------------------------

/// One row of the refusal table: a request that breaks a documented constraint the
/// operation models no error for, the wire `<Code>` it is refused with, and the text the
/// refusal must carry.
struct RefusedCase {
    case: &'static str,
    identifier: &'static str,
    extra: Params,
    code: &'static str,
    says: &'static str,
}

/// The sentence pair Amazon RDS refuses a `DBInstanceIdentifier` with where the value is a
/// token of the member's shape that breaks one of the two hyphen rules: it names the member
/// and states the whole rule, and echoes nothing the request carried.
const NOT_A_VALID_DB_INSTANCE_IDENTIFIER: &str =
    "The parameter DBInstanceIdentifier is not a valid identifier. Identifiers must begin \
     with a letter; must contain only ASCII letters, digits, and hyphens; and must not end \
     with a hyphen or contain two consecutive hyphens.";

/// One row of the not-found table: an operation named a resource that is not stored.
struct NotFoundCase {
    case: &'static str,
    action: &'static str,
    params: Params,
    code: &'static str,
}

#[tokio::test]
async fn documented_constraints_are_refused_at_their_bounds() {
    let router = emulator();

    // Accepted: each published bound of a documented range, and the identifier
    // shapes the constraints admit.
    let accepted: &[(&str, Params)] = &[
        ("lowest Port", params!["Port" => 1150].to_vec()),
        ("highest Port", params!["Port" => 65535].to_vec()),
        (
            "lowest BackupRetentionPeriod",
            params!["BackupRetentionPeriod" => 0].to_vec(),
        ),
        (
            "highest BackupRetentionPeriod",
            params!["BackupRetentionPeriod" => 35].to_vec(),
        ),
        (
            "Multi-AZ without an AvailabilityZone",
            params!["MultiAZ" => true].to_vec(),
        ),
        (
            "an AvailabilityZone without Multi-AZ",
            params!["AvailabilityZone" => "us-west-2c"].to_vec(),
        ),
    ];
    for (index, (case, extra)) in accepted.iter().enumerate() {
        let identifier = format!("emu-ok-{index}");
        expect_ok(
            case,
            create_instance(&router, &identifier, "postgres", "db.t3.micro", extra).await,
        );
    }

    // Refused: one step past each bound, and every identifier shape the
    // `DBInstanceIdentifier` constraints rule out. None of these has a modelled error. A
    // value outside what its own member accepts carries the Query API's
    // `InvalidParameterValue`; the one row that is a parameter COMBINATION rather than a
    // value keeps the channel this service words it through. An instance identifier that is
    // no identifier TOKEN at all — it does not begin with a letter, carries a character
    // outside the alphabet, or is past the member's bound — is worded by its subject
    // ("Invalid database identifier:  <value>"); a token that breaks one of the two hyphen
    // rules states the rule and names the member instead.
    let refused: &[RefusedCase] = &[
        RefusedCase {
            case: "one below the lowest Port",
            identifier: "emu-bad-port-lo",
            extra: params!["Port" => 1149].to_vec(),
            code: "InvalidParameterValue",
            says: "Port",
        },
        RefusedCase {
            case: "one above the highest Port",
            identifier: "emu-bad-port-hi",
            extra: params!["Port" => 65536].to_vec(),
            code: "InvalidParameterValue",
            says: "Port",
        },
        RefusedCase {
            case: "one below the lowest BackupRetentionPeriod",
            identifier: "emu-bad-backup-lo",
            extra: params!["BackupRetentionPeriod" => -1].to_vec(),
            code: "InvalidParameterValue",
            says: "BackupRetentionPeriod",
        },
        RefusedCase {
            case: "one above the highest BackupRetentionPeriod",
            identifier: "emu-bad-backup-hi",
            extra: params!["BackupRetentionPeriod" => 36].to_vec(),
            code: "InvalidParameterValue",
            says: "BackupRetentionPeriod",
        },
        RefusedCase {
            case: "an AvailabilityZone with Multi-AZ",
            identifier: "emu-bad-multi-az",
            extra: params!["MultiAZ" => true, "AvailabilityZone" => "us-west-2a"].to_vec(),
            code: "InvalidParameterCombination",
            says: "AvailabilityZone",
        },
        RefusedCase {
            case: "an identifier that does not start with a letter",
            identifier: "1-emu-bad",
            extra: Vec::new(),
            code: "InvalidParameterValue",
            says: "Invalid database identifier:  1-emu-bad",
        },
        RefusedCase {
            case: "an identifier ending with a hyphen",
            identifier: "emu-bad-",
            extra: Vec::new(),
            code: "InvalidParameterValue",
            says: NOT_A_VALID_DB_INSTANCE_IDENTIFIER,
        },
        RefusedCase {
            case: "an identifier with two consecutive hyphens",
            identifier: "emu--bad",
            extra: Vec::new(),
            code: "InvalidParameterValue",
            says: NOT_A_VALID_DB_INSTANCE_IDENTIFIER,
        },
        RefusedCase {
            case: "an identifier with a character outside the alphabet",
            identifier: "emu_bad",
            extra: Vec::new(),
            code: "InvalidParameterValue",
            says: "Invalid database identifier:  emu_bad",
        },
        RefusedCase {
            case: "an identifier with a multi-byte character",
            identifier: "emu-café",
            extra: Vec::new(),
            code: "InvalidParameterValue",
            says: "Invalid database identifier:  emu-café",
        },
    ];
    for row in refused {
        let body = expect_error(
            row.case,
            create_instance(
                &router,
                row.identifier,
                "postgres",
                "db.t3.micro",
                &row.extra,
            )
            .await,
            400,
            row.code,
        );
        let message = text(&body, "Message");
        assert!(
            message.contains(row.says),
            "case {:?}: the refusal must say {:?}, got {message:?}",
            row.case,
            row.says
        );
    }

    // An identifier one character over the 63-character bound, and one exactly at it.
    let at_bound = format!("e{}", "a".repeat(62));
    expect_ok(
        "an identifier at the length bound",
        create_instance(&router, &at_bound, "postgres", "db.t3.micro", &[]).await,
    );
    let over_bound = format!("e{}", "a".repeat(63));
    let body = expect_error(
        "an identifier one over the length bound",
        create_instance(&router, &over_bound, "postgres", "db.t3.micro", &[]).await,
        400,
        "InvalidParameterValue",
    );
    assert_eq!(
        text(&body, "Message"),
        format!("Invalid database identifier:  {over_bound}"),
        "the refusal must carry the value it rejected, got {:?}",
        text(&body, "Message")
    );
}

#[tokio::test]
async fn an_unknown_identifier_is_not_found_by_every_operation_that_names_one() {
    let router = emulator();
    let cases: &[NotFoundCase] = &[
        NotFoundCase {
            case: "describe an unknown instance",
            action: "DescribeDBInstances",
            params: params!["DBInstanceIdentifier" => "does-not-exist"].to_vec(),
            code: "DBInstanceNotFound",
        },
        NotFoundCase {
            case: "modify an unknown instance",
            action: "ModifyDBInstance",
            params: params![
                "DBInstanceIdentifier" => "does-not-exist",
                "AllocatedStorage" => 50,
            ]
            .to_vec(),
            code: "DBInstanceNotFound",
        },
        NotFoundCase {
            case: "delete an unknown instance",
            action: "DeleteDBInstance",
            params: params![
                "DBInstanceIdentifier" => "does-not-exist",
                "SkipFinalSnapshot" => true,
            ]
            .to_vec(),
            code: "DBInstanceNotFound",
        },
        NotFoundCase {
            case: "snapshot an unknown instance",
            action: "CreateDBSnapshot",
            params: params![
                "DBInstanceIdentifier" => "does-not-exist",
                "DBSnapshotIdentifier" => "emu-snap-orphan",
            ]
            .to_vec(),
            code: "DBInstanceNotFound",
        },
        NotFoundCase {
            case: "describe an unknown snapshot",
            action: "DescribeDBSnapshots",
            params: params!["DBSnapshotIdentifier" => "does-not-exist"].to_vec(),
            code: "DBSnapshotNotFound",
        },
    ];
    for row in cases {
        expect_error(
            row.case,
            call(&router, row.action, &row.params).await,
            404,
            row.code,
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

#[tokio::test]
async fn describe_pages_with_max_records_and_resumes_from_the_marker() {
    let router = emulator();
    const TOTAL: usize = 25;
    for index in 1..=TOTAL {
        let identifier = format!("emu-page-{index:02}");
        expect_ok(
            &identifier,
            create_instance(&router, &identifier, "postgres", "db.t3.micro", &[]).await,
        );
    }

    // `MaxRecords` "Default: 100" — every record fits in one page and no marker is due.
    let (all, marker) = list_instance_identifiers(&router, &[]).await;
    assert_eq!(
        all.len(),
        TOTAL,
        "the default page must hold every record, got {}",
        all.len()
    );
    assert_eq!(marker, None, "no Marker is due for a complete page");

    // At the lowest documented page size the report pages, and the marker resumes
    // exactly where the first page stopped.
    let (first_page, marker) = list_instance_identifiers(&router, &params!["MaxRecords" => 20]).await;
    assert_eq!(
        first_page.len(),
        20,
        "MaxRecords 20 must report 20 records, got {}",
        first_page.len()
    );
    let marker = marker.expect("a Marker is due while records remain");
    let (second_page, no_marker) =
        list_instance_identifiers(&router, &params!["MaxRecords" => 20, "Marker" => marker]).await;
    assert_eq!(
        second_page.len(),
        TOTAL - 20,
        "the resumed page must hold the remaining records, got {}",
        second_page.len()
    );
    assert_eq!(
        no_marker, None,
        "no Marker is due once the last record was reported, got {no_marker:?}"
    );
    let mut paged = first_page;
    paged.extend(second_page);
    assert_eq!(
        paged, all,
        "paging must report every record exactly once, in the same order"
    );

    // The highest documented page size is accepted; one step past either bound is not. A
    // `MaxRecords` outside the documented window is a value the parameter does not accept, so
    // the refusal carries the Query API common error code `InvalidParameterValue`.
    expect_ok(
        "the highest MaxRecords",
        call(&router, "DescribeDBInstances", &params!["MaxRecords" => 100]).await,
    );
    for (case, value) in [("one below the lowest", 19), ("one above the highest", 101)] {
        for action in ["DescribeDBInstances", "DescribeDBSnapshots"] {
            let body = expect_error(
                &format!("{case} MaxRecords for {action}"),
                call(&router, action, &params!["MaxRecords" => value]).await,
                400,
                "InvalidParameterValue",
            );
            assert!(
                text(&body, "Message").contains("MaxRecords"),
                "case {case:?}: the refusal must name MaxRecords, got {:?}",
                text(&body, "Message")
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The transition the clock owes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_creating_instance_reaches_available_on_its_own() {
    let router = emulator();
    expect_ok(
        "create",
        create_instance(&router, "emu-settle", "postgres", "db.t3.micro", &[]).await,
    );

    // Straight after the call the instance is still coming up, and the operations
    // that need it up refuse with the modelled state error.
    let before = describe_one(&router, "before the transition", "emu-settle").await;
    assert_eq!(
        text(&before, "DBInstanceStatus"),
        "creating",
        "a caller reading straight after the create must still see `creating`"
    );
    expect_error(
        "snapshot while creating",
        call(
            &router,
            "CreateDBSnapshot",
            &params![
                "DBInstanceIdentifier" => "emu-settle",
                "DBSnapshotIdentifier" => "emu-settle-snap",
            ],
        )
        .await,
        400,
        "InvalidDBInstanceState",
    );
    expect_error(
        "modify while creating",
        call(
            &router,
            "ModifyDBInstance",
            &params!["DBInstanceIdentifier" => "emu-settle", "AllocatedStorage" => 50],
        )
        .await,
        400,
        "InvalidDBInstanceState",
    );

    wait_for_settle();
    let after = describe_one(&router, "after the transition", "emu-settle").await;
    assert_eq!(
        text(&after, "DBInstanceStatus"),
        "available",
        "the instance must reach `available` once the transition is due"
    );
    // Reading again does not move it on.
    let again = describe_one(&router, "read twice", "emu-settle").await;
    assert_eq!(
        text(&again, "DBInstanceStatus"),
        "available",
        "a second read must report the same status"
    );
}

// ---------------------------------------------------------------------------
// ModifyDBInstance
// ---------------------------------------------------------------------------

#[tokio::test]
async fn modify_writes_every_supplied_setting_through_to_the_store() {
    let router = emulator();
    create_available_instance(
        &router,
        "emu-mod",
        "postgres",
        "db.t3.micro",
        &params!["AllocatedStorage" => 20, "MasterUsername" => "pgadmin"],
    )
    .await;

    // Each row changes one documented member; the following describe must report the
    // new value out of the store, never an echo of the request.
    let rows: &[(&str, &str, &str, &str)] = &[
        ("storage grows", "AllocatedStorage", "50", "AllocatedStorage"),
        (
            "backup retention changes",
            "BackupRetentionPeriod",
            "14",
            "BackupRetentionPeriod",
        ),
        (
            "the instance class changes",
            "DBInstanceClass",
            "db.m5.2xlarge",
            "DBInstanceClass",
        ),
        ("multi-AZ is turned on", "MultiAZ", "true", "MultiAZ"),
        ("the engine version changes", "EngineVersion", "17.2", "EngineVersion"),
        ("the storage type changes", "StorageType", "io1", "StorageType"),
        (
            "the backup window changes",
            "PreferredBackupWindow",
            "03:00-04:00",
            "PreferredBackupWindow",
        ),
        (
            "an upper storage bound is set",
            "MaxAllocatedStorage",
            "500",
            "MaxAllocatedStorage",
        ),
        ("the port changes", "DBPortNumber", "15432", "Port"),
    ];
    for (case, member, value, reported_as) in rows {
        let modified = expect_ok(
            case,
            call(
                &router,
                "ModifyDBInstance",
                &params![
                    "DBInstanceIdentifier" => "emu-mod",
                    "ApplyImmediately" => true,
                    *member => *value,
                ],
            )
            .await,
        );
        let modified = the_instance(case, &modified);
        assert_eq!(
            text(&modified, "DBInstanceStatus"),
            "modifying",
            "case {case:?}: a modification leaves the instance `modifying`"
        );
        wait_for_settle();

        let read_back = describe_one(&router, case, "emu-mod").await;
        let actual = optional_text(&read_back, reported_as);
        if *reported_as == "Port" {
            // `DBPortNumber` sets the port the instance is CONFIGURED to listen on, which
            // the record keeps; the port it PUBLISHES is the one the core assigned its own
            // engine listener (the core owns the port range), so what has to hold after a
            // port change is that the published endpoint is still a port a client can
            // reach.
            assert!(
                actual
                    .as_deref()
                    .is_some_and(|port| port.parse::<u16>().is_ok_and(|port| port > 0)),
                "case {case:?}: the published port must stay the port the instance's \
                 engine listener is bound to, got {actual:?}"
            );
        } else {
            assert_eq!(
                actual.as_deref(),
                Some(*value),
                "case {case:?}: the store must report {reported_as} as {value:?}, got {actual:?}"
            );
        }
        assert_eq!(
            text(&read_back, "DBInstanceStatus"),
            "available",
            "case {case:?}: the instance must settle back to `available`"
        );
    }

    // The settings no row touched are untouched.
    let read_back = describe_one(&router, "untouched settings", "emu-mod").await;
    assert_eq!(
        optional_text(&read_back, "MasterUsername").as_deref(),
        Some("pgadmin"),
        "a modification must not disturb a setting the request did not name"
    );

    // A modification the caller did not ask to apply immediately is staged for the next
    // maintenance window: the live attribute keeps the value the instance is running
    // under, and the staged one is reported as pending. The size is one the storage type
    // the rows above left the instance on takes — Provisioned IOPS SSD starts at 100 GiB,
    // and `DescribeValidDBInstanceModifications` reports exactly that, which is the bound
    // this operation enforces.
    let live_storage = optional_text(
        &describe_one(&router, "before a deferred modification", "emu-mod").await,
        "AllocatedStorage",
    );
    let deferred = expect_ok(
        "deferred modification",
        call(
            &router,
            "ModifyDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-mod",
                "ApplyImmediately" => false,
                "AllocatedStorage" => 150,
            ],
        )
        .await,
    );
    let deferred = the_instance("deferred modification", &deferred);
    assert_eq!(
        optional_text(&deferred, "AllocatedStorage"),
        live_storage,
        "a deferred modification must leave the live storage alone, got {:?}",
        optional_text(&deferred, "AllocatedStorage")
    );
    let pending = subtrees(&deferred, "PendingModifiedValues");
    assert_eq!(
        pending.len(),
        1,
        "a deferred modification reports pending modified values, got {pending:?}"
    );
    assert_eq!(
        text(&pending[0], "AllocatedStorage"),
        "150",
        "a deferred modification must report the staged storage as pending"
    );
}

#[tokio::test]
async fn modify_renames_an_instance_and_refuses_a_name_already_taken() {
    let router = emulator();
    create_available_instance(&router, "emu-rename-a", "postgres", "db.t3.micro", &[]).await;
    create_available_instance(&router, "emu-rename-b", "postgres", "db.t3.micro", &[]).await;

    expect_error(
        "rename onto an existing identifier",
        call(
            &router,
            "ModifyDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-rename-a",
                "NewDBInstanceIdentifier" => "emu-rename-b",
            ],
        )
        .await,
        400,
        "DBInstanceAlreadyExists",
    );
    expect_error(
        "rename to an identifier the constraints rule out",
        call(
            &router,
            "ModifyDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-rename-a",
                "NewDBInstanceIdentifier" => "9-bad-name",
            ],
        )
        .await,
        400,
        "InvalidParameterValue",
    );

    let renamed = expect_ok(
        "rename",
        call(
            &router,
            "ModifyDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-rename-a",
                // "When you change the DB instance identifier, an instance reboot occurs
                // immediately if you enable ApplyImmediately, or will occur during the
                // next maintenance window if you disable ApplyImmediately" — and what
                // this test is about is the rename itself, so it asks for it now. The
                // deferred form is covered by `pending_modified_values`.
                "ApplyImmediately" => true,
                "NewDBInstanceIdentifier" => "Emu-Rename-C",
            ],
        )
        .await,
    );
    let renamed = the_instance("rename", &renamed);
    assert_eq!(
        text(&renamed, "DBInstanceIdentifier"),
        "emu-rename-c",
        "the new identifier is stored as a lowercase string too"
    );
    assert_eq!(
        text(&renamed, "DBInstanceArn"),
        format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:emu-rename-c"),
        "the ARN must follow the rename"
    );
    // A rename does not move the endpoint: the instance keeps the engine listener it was
    // created with, so a client reconnects to the same address under the new name. (The
    // DNS-form endpoint, which follows the identifier, is what an instance with no
    // listener publishes — see `create_persists_every_setting_and_describe_reads_it_back`.)
    let renamed_address = text(&renamed, "Address");
    assert!(
        renamed_address
            .parse::<std::net::IpAddr>()
            .is_ok_and(|host| host.is_loopback()),
        "the renamed instance must keep publishing its engine listener's host, got \
         {renamed_address:?}"
    );
    wait_for_settle();

    expect_error(
        "the old identifier after a rename",
        call(
            &router,
            "DescribeDBInstances",
            &params!["DBInstanceIdentifier" => "emu-rename-a"],
        )
        .await,
        404,
        "DBInstanceNotFound",
    );
    let read_back = describe_one(&router, "the new identifier", "emu-rename-c").await;
    assert_eq!(
        text(&read_back, "DBInstanceIdentifier"),
        "emu-rename-c",
        "the renamed record must be readable under its new identifier"
    );
}

// ---------------------------------------------------------------------------
// Snapshots
// ---------------------------------------------------------------------------

/// One row of the snapshot table: the source instance's settings a snapshot copies.
struct SnapshotCase {
    case: &'static str,
    instance: &'static str,
    snapshot: &'static str,
    engine: &'static str,
    extra: &'static [(&'static str, &'static str)],
    expected_port: &'static str,
}

const SNAPSHOT_CASES: &[SnapshotCase] = &[
    SnapshotCase {
        case: "postgres with every copied setting supplied",
        instance: "emu-snap-pg",
        snapshot: "emu-snap-pg-1",
        engine: "postgres",
        extra: &[
            ("AllocatedStorage", "20"),
            ("MasterUsername", "pgadmin"),
            ("EngineVersion", "16.4"),
            ("AvailabilityZone", "us-west-2b"),
            ("StorageType", "gp3"),
        ],
        expected_port: "5432",
    },
    SnapshotCase {
        case: "mysql on an explicit port",
        instance: "emu-snap-my",
        snapshot: "Emu-Snap-My-1",
        engine: "mysql",
        extra: &[
            ("AllocatedStorage", "200"),
            ("MasterUsername", "root"),
            ("Port", "13306"),
            ("StorageType", "io1"),
            ("Iops", "3000"),
        ],
        expected_port: "13306",
    },
    SnapshotCase {
        case: "oracle on its default port",
        instance: "emu-snap-ora",
        snapshot: "emu-snap-ora-1",
        engine: "oracle-se2",
        extra: &[("AllocatedStorage", "40"), ("MasterUsername", "oraadmin")],
        expected_port: "1521",
    },
];

#[tokio::test]
async fn a_snapshot_copies_the_source_instance_and_reads_back() {
    let router = emulator();
    for row in SNAPSHOT_CASES {
        let extra: Vec<(&str, String)> = row
            .extra
            .iter()
            .map(|(name, value)| (*name, (*value).to_string()))
            .collect();
        create_available_instance(&router, row.instance, row.engine, "db.m5.large", &extra).await;

        let created = expect_ok(
            row.case,
            call(
                &router,
                "CreateDBSnapshot",
                &params![
                    "DBInstanceIdentifier" => row.instance,
                    "DBSnapshotIdentifier" => row.snapshot,
                    "Tags.Tag.1.Key" => "purpose",
                    "Tags.Tag.1.Value" => "test",
                ],
            )
            .await,
        );
        assert_eq!(
            text(&created, "Status"),
            "creating",
            "case {:?}: a snapshot RDS has just started is `creating`",
            row.case
        );
        assert_eq!(
            text(&created, "PercentProgress"),
            "0",
            "case {:?}: a snapshot that has just started has made no progress",
            row.case
        );
        wait_for_settle();

        // Readable back through DescribeDBSnapshots, filtered by the source instance.
        let listed = expect_ok(
            row.case,
            call(
                &router,
                "DescribeDBSnapshots",
                &params!["DBInstanceIdentifier" => row.instance],
            )
            .await,
        );
        let snapshots = subtrees(&listed, "DBSnapshot");
        assert_eq!(
            snapshots.len(),
            1,
            "case {:?}: expected one snapshot for {}, got {}",
            row.case,
            row.instance,
            snapshots.len()
        );
        let snapshot = &snapshots[0];

        // The source instance as the store holds it — what the snapshot must copy.
        let source = describe_one(&router, row.case, row.instance).await;
        for member in [
            "Engine",
            "EngineVersion",
            "AllocatedStorage",
            "MasterUsername",
            "AvailabilityZone",
            "StorageType",
            "Iops",
            "DbiResourceId",
        ] {
            let copied = optional_text(snapshot, member);
            let original = optional_text(&source, member);
            assert_eq!(
                copied, original,
                "case {:?}: the snapshot reported {member} as {copied:?}, but the source \
                 instance holds {original:?}",
                row.case
            );
        }
        let port = text(snapshot, "Port");
        assert_eq!(
            port, row.expected_port,
            "case {:?}: the snapshot must copy the source port, got {port:?}",
            row.case
        );
        assert_eq!(
            text(snapshot, "Status"),
            "available",
            "case {:?}: the snapshot must reach `available`",
            row.case
        );
        assert_eq!(
            text(snapshot, "PercentProgress"),
            "100",
            "case {:?}: an available snapshot is fully copied",
            row.case
        );
        assert_eq!(
            text(snapshot, "SnapshotType"),
            "manual",
            "case {:?}: a snapshot the caller asked for is `manual`",
            row.case
        );
        assert_eq!(
            text(snapshot, "DBSnapshotArn"),
            format!("arn:aws:rds:{REGION}:{ACCOUNT}:snapshot:{}", row.snapshot),
            "case {:?}: unexpected snapshot ARN",
            row.case
        );
        assert_eq!(
            text(snapshot, "InstanceCreateTime"),
            text(&source, "InstanceCreateTime"),
            "case {:?}: the snapshot must copy the source instance's creation time",
            row.case
        );
        assert!(
            !text(snapshot, "SnapshotCreateTime").is_empty(),
            "case {:?}: the snapshot must carry its own creation time",
            row.case
        );
        assert_eq!(
            texts(snapshot, "Key"),
            vec!["purpose".to_string()],
            "case {:?}: the snapshot must carry the tags the request supplied",
            row.case
        );

        // A second snapshot under the same identifier is refused.
        expect_error(
            row.case,
            call(
                &router,
                "CreateDBSnapshot",
                &params![
                    "DBInstanceIdentifier" => row.instance,
                    "DBSnapshotIdentifier" => row.snapshot,
                ],
            )
            .await,
            400,
            "DBSnapshotAlreadyExists",
        );
    }

    // A snapshot is a copy, not a view: changing the instance afterwards does not
    // rewrite what the snapshot reports.
    expect_ok(
        "grow the source instance",
        call(
            &router,
            "ModifyDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-snap-pg",
                "ApplyImmediately" => true,
                "AllocatedStorage" => 500,
            ],
        )
        .await,
    );
    wait_for_settle();
    let listed = expect_ok(
        "the snapshot after the instance grew",
        call(
            &router,
            "DescribeDBSnapshots",
            &params!["DBSnapshotIdentifier" => "emu-snap-pg-1"],
        )
        .await,
    );
    assert_eq!(
        text(&listed, "AllocatedStorage"),
        "20",
        "the snapshot must keep the storage the instance had when it was taken"
    );
    assert_eq!(
        text(&describe_one(&router, "the grown instance", "emu-snap-pg").await, "AllocatedStorage"),
        "500",
        "the instance itself must report its new storage"
    );

    // Selecting snapshots: by type, by the documented filters, and by nothing.
    let (all, _) = snapshot_identifiers(&router, &[]).await;
    let mut expected: Vec<String> = SNAPSHOT_CASES
        .iter()
        .map(|row| row.snapshot.to_string())
        .collect();
    expected.sort();
    assert_eq!(
        all, expected,
        "the unfiltered report must list every stored snapshot in identifier order"
    );
    let (manual, _) = snapshot_identifiers(&router, &params!["SnapshotType" => "manual"]).await;
    assert_eq!(
        manual, expected,
        "every stored snapshot is `manual`, so the manual report lists them all"
    );
    let (automated, _) =
        snapshot_identifiers(&router, &params!["SnapshotType" => "automated"]).await;
    assert!(
        automated.is_empty(),
        "this store holds no automated snapshots, got {automated:?}"
    );
    let (by_engine, _) = snapshot_identifiers(
        &router,
        &params!["Filters.Filter.1.Name" => "engine", "Filters.Filter.1.Values.Value.1" => "mysql"],
    )
    .await;
    assert_eq!(
        by_engine,
        vec!["Emu-Snap-My-1".to_string()],
        "the engine filter must select only the mysql snapshot"
    );
    let (by_arn, _) = snapshot_identifiers(
        &router,
        &params![
            "Filters.Filter.1.Name" => "db-instance-id",
            "Filters.Filter.1.Values.Value.1" => format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:emu-snap-ora"),
        ],
    )
    .await;
    assert_eq!(
        by_arn,
        vec!["emu-snap-ora-1".to_string()],
        "db-instance-id must accept a DB instance ARN as well as an identifier"
    );
    let body = expect_error(
        "an unsupported snapshot filter",
        call(
            &router,
            "DescribeDBSnapshots",
            &params![
                "Filters.Filter.1.Name" => "not-a-filter",
                "Filters.Filter.1.Values.Value.1" => "x",
            ],
        )
        .await,
        400,
        "InvalidParameterValue",
    );
    assert!(
        text(&body, "Message").contains("not-a-filter"),
        "the refusal must name the filter it did not recognise, got {:?}",
        text(&body, "Message")
    );
}

/// The snapshot identifiers `DescribeDBSnapshots` reports, and its `Marker`.
async fn snapshot_identifiers(
    router: &MultiServiceRouter,
    extra: &[(&str, String)],
) -> (Vec<String>, Option<String>) {
    let body = expect_ok(
        "list snapshots",
        call(router, "DescribeDBSnapshots", extra).await,
    );
    let identifiers = subtrees(&body, "DBSnapshot")
        .iter()
        .map(|snapshot| text(snapshot, "DBSnapshotIdentifier"))
        .collect();
    (identifiers, optional_text(&body, "Marker"))
}

/// One row of the filter table: one documented filter name, one value, and the
/// instances it must select.
struct FilterCase {
    case: &'static str,
    name: &'static str,
    value: String,
    expected: &'static [&'static str],
}

#[tokio::test]
async fn instance_filters_select_by_every_documented_name() {
    let router = emulator();
    create_available_instance(
        &router,
        "emu-filter-pg",
        "postgres",
        "db.t3.micro",
        &params!["Domain" => "d-emu1"],
    )
    .await;
    create_available_instance(&router, "emu-filter-my", "mysql", "db.t3.micro", &[]).await;

    let resource_id = text(
        &describe_one(&router, "the postgres instance", "emu-filter-pg").await,
        "DbiResourceId",
    );
    let cases: &[FilterCase] = &[
        FilterCase {
            case: "by identifier",
            name: "db-instance-id",
            value: "emu-filter-my".to_string(),
            expected: &["emu-filter-my"],
        },
        FilterCase {
            case: "by ARN",
            name: "db-instance-id",
            value: format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:emu-filter-pg"),
            expected: &["emu-filter-pg"],
        },
        FilterCase {
            case: "by engine",
            name: "engine",
            value: "postgres".to_string(),
            expected: &["emu-filter-pg"],
        },
        FilterCase {
            case: "by resource id",
            name: "dbi-resource-id",
            value: resource_id,
            expected: &["emu-filter-pg"],
        },
        FilterCase {
            // `CreateDBInstance` resolves `DBClusterIdentifier` against this service's DB
            // cluster records before admitting the instance, and no `CreateDBCluster` ran
            // in THIS test, so no instance here belongs to a cluster and this filter
            // selects nothing. A cluster's members are pinned in `db_clusters`, and the
            // refusal for an identifier that resolves to nothing in `docdb_query_surface`.
            case: "by cluster",
            name: "db-cluster-id",
            value: "emu-cluster".to_string(),
            expected: &[],
        },
        FilterCase {
            case: "by domain",
            name: "domain",
            value: "d-emu1".to_string(),
            expected: &["emu-filter-pg"],
        },
        FilterCase {
            case: "by an engine nothing uses",
            name: "engine",
            value: "mariadb".to_string(),
            expected: &[],
        },
    ];
    for row in cases {
        let (identifiers, _) = list_instance_identifiers(
            &router,
            &params![
                "Filters.Filter.1.Name" => row.name,
                "Filters.Filter.1.Values.Value.1" => row.value,
            ],
        )
        .await;
        assert_eq!(
            identifiers, row.expected,
            "case {:?}: the {} filter reported {identifiers:?}",
            row.case, row.name
        );
    }

    let body = expect_error(
        "an unsupported instance filter",
        call(
            &router,
            "DescribeDBInstances",
            &params![
                "Filters.Filter.1.Name" => "db-instance-name",
                "Filters.Filter.1.Values.Value.1" => "emu-filter-pg",
            ],
        )
        .await,
        400,
        "InvalidParameterValue",
    );
    assert!(
        text(&body, "Message").contains("db-instance-name"),
        "the refusal must name the filter it did not recognise, got {:?}",
        text(&body, "Message")
    );
}

// ---------------------------------------------------------------------------
// DeleteDBInstance
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_removes_the_instance_and_can_take_a_final_snapshot() {
    let router = emulator();
    create_available_instance(
        &router,
        "emu-del-skip",
        "postgres",
        "db.t3.micro",
        &params!["AllocatedStorage" => 20, "MasterUsername" => "pgadmin"],
    )
    .await;
    create_available_instance(
        &router,
        "emu-del-final",
        "mysql",
        "db.m5.large",
        &params!["AllocatedStorage" => 80, "MasterUsername" => "root"],
    )
    .await;
    create_available_instance(
        &router,
        "emu-del-protected",
        "postgres",
        "db.t3.micro",
        &params!["DeletionProtection" => true],
    )
    .await;

    // The documented parameter combinations this operation models no error for, and the
    // one row that is a VALUE outside what `FinalDBSnapshotIdentifier` accepts: the
    // combinations carry the Query API's `InvalidParameterCombination` and the value its
    // `InvalidParameterValue`.
    let refusals: &[(&str, Params, &str)] = &[
        (
            "both skip and a final snapshot identifier",
            params![
                "DBInstanceIdentifier" => "emu-del-skip",
                "SkipFinalSnapshot" => true,
                "FinalDBSnapshotIdentifier" => "emu-del-skip-final",
            ]
            .to_vec(),
            "InvalidParameterCombination",
        ),
        (
            "neither skip nor a final snapshot identifier",
            params!["DBInstanceIdentifier" => "emu-del-skip"].to_vec(),
            "InvalidParameterCombination",
        ),
        (
            "a final snapshot identifier the constraints rule out",
            params![
                "DBInstanceIdentifier" => "emu-del-skip",
                "FinalDBSnapshotIdentifier" => "1-bad-final",
            ]
            .to_vec(),
            "InvalidParameterValue",
        ),
    ];
    for (case, params, code) in refusals {
        let body = expect_error(case, call(&router, "DeleteDBInstance", params).await, 400, code);
        assert!(
            text(&body, "Message").contains("FinalDBSnapshotIdentifier"),
            "case {case:?}: the refusal must name the member, got {:?}",
            text(&body, "Message")
        );
    }

    // Deletion protection holds the instance: a setting the request conflicts with, which
    // is a parameter refusal rather than a state fault.
    expect_error(
        "a protected instance",
        call(
            &router,
            "DeleteDBInstance",
            &params!["DBInstanceIdentifier" => "emu-del-protected", "SkipFinalSnapshot" => true],
        )
        .await,
        400,
        "InvalidParameterCombination",
    );
    // A status the clock has yet to settle does NOT refuse a delete: nothing has to
    // be up for RDS to tear an instance down. `db_instance_deletion` spans the
    // statuses a delete is accepted from; this row keeps this refusal suite honest
    // about which refusals are real.
    expect_ok(
        "an instance that is still creating",
        create_instance(&router, "emu-del-creating", "postgres", "db.t3.micro", &[]).await,
    );
    expect_ok(
        "delete while creating",
        call(
            &router,
            "DeleteDBInstance",
            &params!["DBInstanceIdentifier" => "emu-del-creating", "SkipFinalSnapshot" => true],
        )
        .await,
    );
    let still_there = describe_one(&router, "a protected instance", "emu-del-protected").await;
    assert_eq!(
        text(&still_there, "DeletionProtection"),
        "true",
        "a refused delete must leave the instance exactly as it was"
    );

    // Skipping the final snapshot: the instance goes, and nothing is stored for it.
    let deleted = expect_ok(
        "delete, skipping the final snapshot",
        call(
            &router,
            "DeleteDBInstance",
            &params!["DBInstanceIdentifier" => "emu-del-skip", "SkipFinalSnapshot" => true],
        )
        .await,
    );
    assert_eq!(
        text(&the_instance("delete, skipping", &deleted), "DBInstanceStatus"),
        "deleting",
        "a delete reports the instance as `deleting`"
    );
    // The instance reports `deleting` while the clock owes its removal, and is not
    // found at all once that removal is due.
    assert_eq!(
        text(
            &describe_one(&router, "a read inside the deleting window", "emu-del-skip").await,
            "DBInstanceStatus"
        ),
        "deleting",
        "a read inside the deleting window must report `deleting`"
    );
    wait_for_settle();
    for attempt in ["the first read after the removal", "and the second"] {
        expect_error(
            attempt,
            call(
                &router,
                "DescribeDBInstances",
                &params!["DBInstanceIdentifier" => "emu-del-skip"],
            )
            .await,
            404,
            "DBInstanceNotFound",
        );
    }
    expect_error(
        "deleting twice",
        call(
            &router,
            "DeleteDBInstance",
            &params!["DBInstanceIdentifier" => "emu-del-skip", "SkipFinalSnapshot" => true],
        )
        .await,
        404,
        "DBInstanceNotFound",
    );
    let (identifiers, _) = list_instance_identifiers(&router, &[]).await;
    assert!(
        !identifiers.contains(&"emu-del-skip".to_string()),
        "the deleted instance must be gone from the unfiltered report, got {identifiers:?}"
    );
    let (snapshots, _) = snapshot_identifiers(&router, &[]).await;
    assert!(
        snapshots.is_empty(),
        "skipping the final snapshot must store none, got {snapshots:?}"
    );

    // Taking the final snapshot: the instance goes and the snapshot stays, carrying
    // the settings the instance had.
    let source = describe_one(&router, "the instance before its final snapshot", "emu-del-final")
        .await;
    expect_ok(
        "delete with a final snapshot",
        call(
            &router,
            "DeleteDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-del-final",
                "FinalDBSnapshotIdentifier" => "emu-del-final-snap",
            ],
        )
        .await,
    );
    wait_for_settle();
    let listed = expect_ok(
        "the final snapshot",
        call(
            &router,
            "DescribeDBSnapshots",
            &params!["DBSnapshotIdentifier" => "emu-del-final-snap"],
        )
        .await,
    );
    let snapshot = &subtrees(&listed, "DBSnapshot")[0];
    for member in ["Engine", "AllocatedStorage", "MasterUsername", "DbiResourceId"] {
        assert_eq!(
            optional_text(snapshot, member),
            optional_text(&source, member),
            "the final snapshot must copy {member} from the instance it was taken from"
        );
    }
    assert_eq!(
        text(snapshot, "DBInstanceIdentifier"),
        "emu-del-final",
        "the final snapshot must name the instance it came from"
    );
    expect_error(
        "the instance after its final snapshot",
        call(
            &router,
            "DescribeDBInstances",
            &params!["DBInstanceIdentifier" => "emu-del-final"],
        )
        .await,
        404,
        "DBInstanceNotFound",
    );
}
