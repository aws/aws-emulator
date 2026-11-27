// SPDX-License-Identifier: CC0-1.0

//! The ADDRESSES an environment publishes, over the service's own HTTP surface.
//!
//! The rule under test: both addresses an environment publishes are HOSTS,
//! carrying no port and no path. `EnvironmentDescription$CNAME` ("the URL to the
//! CNAME for this environment") is the fully qualified DNS name the environment's
//! CNAME prefix forms in its Region, under a sub-domain of that environment's own,
//! in the zone this emulator answers in: its FIRST dot-label is the prefix the
//! launch was given, so the prefix is recoverable from it and can be handed
//! straight back to `CheckDNSAvailability` — which REPORTS the name that prefix
//! forms in Elastic Beanstalk's own zone, a name rather than an address.
//! `EndpointURL` ("for load-balanced, autoscaling environments, the URL to the
//! LoadBalancer. For single-instance environments, the IP address of the
//! instance") is the address of what answers BEHIND that name: the host of the
//! load balancer the environment's own inventory holds, or the address its single
//! instance answers at. `SwapEnvironmentCNAMEs` exchanges the NAMES, which
//! exchanges which application answers at each of them, and leaves each
//! environment's own address where it was.
//!
//! The PORT and the PATH a request needs are resolved at the listener the plane is
//! served on, not published into either member: a client that resolves a published
//! name reaches that socket, and a client that resolves nothing but the emulator's
//! own endpoint reaches the same plane by the path naming the Region and the prefix.
//!
//! The table spans what the rule generalises over: the Region the environment is
//! launched in, whether the launch supplies the CNAME prefix or leaves it to be
//! generated from the environment's own name, the prefix's length at both ends of
//! the `DNSCnamePrefix` `@length(4, 63)` bound and in mixed case (a CNAME prefix
//! is a host name label, and DNS compares labels case-insensitively), an
//! environment name carrying the delimiters that would move the authority of the
//! address the name IS, and whether the environment is launched behind a load
//! balancer or as a single instance — which is what decides which form its
//! `EndpointURL` takes.
//!
//! The HTTP harness (the router, the form encoder, the XML element reader, the
//! object-storage upload and the unsigned GET) is the one `lifecycle.rs` owns in
//! this same test binary.

use crate::application_version_processing::zip_of;
use crate::harness::{wait_for_status, Answer, Emulator, SOLUTION_STACK};
use crate::lifecycle::{assert_published_cname, PUBLISHED_DOMAIN, TEST_REGION};

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

/// The application every environment below is launched under.
const APPLICATION: &str = "published-name-app";

/// The bucket prefix the suite below uploads each source bundle into: one bucket
/// per bundle, because creating a bucket that already exists is refused.
const BUNDLE_BUCKET_PREFIX: &str = "eb-published-name-bundles";

/// The modelled length bounds of `DNSCname`, the shape `EnvironmentDescription$
/// CNAME` targets, from `smithy_query(operation="DescribeEnvironments")` ->
/// `{CNAME: DNSCname @length 1..255}`.
const DNS_CNAME_LENGTH: std::ops::RangeInclusive<usize> = 1..=255;

/// The configuration-option namespace and option that say whether an environment
/// sits behind a load balancer, from Elastic Beanstalk's published
/// configuration-option reference: "EnvironmentType: Set to SingleInstance to
/// launch one EC2 instance with no load balancer", whose documented default is
/// `LoadBalanced`. Spelled as a request sends it, the way the suites beside this
/// one send it.
const NAMESPACE_ENVIRONMENT: &str = "aws:elasticbeanstalk:environment";
/// See [`NAMESPACE_ENVIRONMENT`].
const OPTION_ENVIRONMENT_TYPE: &str = "EnvironmentType";

/// One launch, and what its published addresses must then be read back as.
struct NameRow {
    case: &'static str,
    /// The Region the launch is signed for, which is the name's SECOND label.
    region: &'static str,
    environment_name: &'static str,
    /// The `CNAMEPrefix` the launch supplies, or `None` for a launch that leaves
    /// it to be generated ("if not specified, the CNAME is generated
    /// automatically by appending a random alphanumeric string to the
    /// environment name").
    cname_prefix: Option<String>,
    /// The `aws:elasticbeanstalk:environment` / `EnvironmentType` the launch sets,
    /// or `None` to leave the documented default (`LoadBalanced`) in force — which
    /// is what decides whether the environment has a load balancer in front of it,
    /// and so which form its `EndpointURL` takes.
    environment_type: Option<&'static str>,
}

/// The rows, one per form the rule generalises over.
fn rows() -> Vec<NameRow> {
    vec![
        NameRow {
            case: "a supplied prefix",
            region: TEST_REGION,
            environment_name: "named-env",
            cname_prefix: Some("published-name".to_string()),
            environment_type: None,
        },
        NameRow {
            case: "a generated prefix",
            region: TEST_REGION,
            environment_name: "generated-env",
            cname_prefix: None,
            environment_type: None,
        },
        NameRow {
            case: "another Region",
            region: "eu-central-1",
            environment_name: "other-region-env",
            // The same prefix in two Regions is two names, so the Region label is
            // what tells them apart.
            cname_prefix: Some("published-name".to_string()),
            environment_type: None,
        },
        NameRow {
            case: "a prefix at the length trait's minimum",
            region: TEST_REGION,
            environment_name: "min-env",
            cname_prefix: Some("abcd".to_string()),
            environment_type: None,
        },
        NameRow {
            case: "a prefix at the length trait's maximum",
            region: TEST_REGION,
            environment_name: "max-env",
            cname_prefix: Some("z".repeat(63)),
            environment_type: None,
        },
        NameRow {
            case: "a prefix in mixed case",
            region: TEST_REGION,
            environment_name: "mixed-case-env",
            cname_prefix: Some("Published-NAME-Mixed".to_string()),
            environment_type: None,
        },
        NameRow {
            case: "a single-instance environment",
            region: TEST_REGION,
            environment_name: "single-instance-env",
            cname_prefix: Some("single-instance".to_string()),
            environment_type: Some(eb::state::ENVIRONMENT_TYPE_SINGLE_INSTANCE),
        },
        NameRow {
            case: "an environment that names the load-balanced default itself",
            region: TEST_REGION,
            environment_name: "load-balanced-env",
            cname_prefix: Some("load-balanced".to_string()),
            environment_type: Some(eb::state::ENVIRONMENT_TYPE_LOAD_BALANCED),
        },
        NameRow {
            // The published address is the AUTHORITY a client dials, so a value
            // the request carried may not move it: an environment name carrying
            // the delimiters that would ('.' a label, ':' the port, '/' a path)
            // reaches the name folded into one label. `EnvironmentName` is bounded
            // in LENGTH only, and a launch that supplies no `CNAMEPrefix` derives
            // the prefix from it, so this is the widest value that reaches the
            // leading label.
            case: "an environment name carrying an authority delimiter",
            region: TEST_REGION,
            environment_name: "evil.example:8080/x",
            cname_prefix: None,
            environment_type: None,
        },
    ]
}

/// Call `action`, failing the named case unless it answered 200.
async fn ok(emulator: &Emulator, case: &str, action: &str, params: &[(&str, &str)]) -> Answer {
    let answer = emulator.call(action, params).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from {action}, got {} and body {}",
        answer.status, answer.body
    );
    answer
}

/// The same, signed for `region`.
///
/// `pub(crate)` for the sibling suite that drives the same launches in the same
/// binary (`published_address_hosts.rs`), so one call-and-assert serves both.
pub(crate) async fn ok_in(
    emulator: &Emulator,
    case: &str,
    region: &str,
    action: &str,
    params: &[(&str, &str)],
) -> Answer {
    let answer = emulator.call_in_region(region, action, params).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from {action}, got {} and body {}",
        answer.status, answer.body
    );
    answer
}

/// Hold `value` to the form every published address shares: a dot-separated host
/// carrying neither a port separator nor a path separator, with no empty label at
/// either end.
fn assert_is_a_host(case: &str, member: &str, value: &str) {
    assert!(
        !value.contains(':') && !value.contains('/'),
        "case {case:?}: {member} carries neither ':' nor '/', got {value:?}"
    );
    assert!(
        !value.starts_with('.') && !value.ends_with('.'),
        "case {case:?}: {member} must carry no empty leading or trailing label, got {value:?}"
    );
    assert!(
        !value.is_empty(),
        "case {case:?}: {member} must name a host, got {value:?}"
    );
}

/// The name of the load balancer the environment `environment_name` holds, read
/// back through `DescribeEnvironmentResources` — the inventory the environment is
/// made of, which is where its `EndpointURL` comes from. `None` for an environment
/// with no load balancer in front of it.
async fn load_balancer_of(
    emulator: &Emulator,
    case: &str,
    region: &str,
    environment_name: &str,
) -> Option<String> {
    let resources = ok_in(
        emulator,
        case,
        region,
        "DescribeEnvironmentResources",
        &[("EnvironmentName", environment_name)],
    )
    .await;
    resources
        .value("EnvironmentResources/LoadBalancers/member/Name")
        .map(str::to_string)
}

/// RULE: the CNAME an environment publishes is the fully qualified DNS name its
/// prefix forms in its Region — the name `CheckDNSAvailability` reports for that
/// prefix, with the prefix as its first label and no port or path anywhere — and
/// the `EndpointURL` beside it is the host of what answers behind that name: the
/// environment's own load balancer, or the address its single instance answers at.
#[tokio::test]
async fn an_environment_publishes_the_dns_name_its_prefix_forms() {
    let emulator = Emulator::new();
    ok(
        &emulator,
        "setup",
        "CreateApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;

    for row in rows() {
        let case = row.case;
        let mut params: Vec<(&str, &str)> = vec![
            ("ApplicationName", APPLICATION),
            ("EnvironmentName", row.environment_name),
            ("SolutionStackName", SOLUTION_STACK.as_str()),
        ];
        if let Some(prefix) = row.cname_prefix.as_deref() {
            params.push(("CNAMEPrefix", prefix));
        }
        if let Some(environment_type) = row.environment_type {
            params.push(("OptionSettings.member.1.Namespace", NAMESPACE_ENVIRONMENT));
            params.push(("OptionSettings.member.1.OptionName", OPTION_ENVIRONMENT_TYPE));
            params.push(("OptionSettings.member.1.Value", environment_type));
        }
        let launched = ok_in(&emulator, case, row.region, "CreateEnvironment", &params).await;
        let environment_id = launched.expect("EnvironmentId", case).to_string();
        // The name is published once the provisioning the launch started has
        // finished, so the launch itself reports none and the CNAME is read off
        // the environment.
        assert!(
            launched.value("CNAME").is_none(),
            "case {case:?}: a launch still provisioning reports no CNAME, got {}",
            launched.body
        );
        wait_for_status(&emulator, case, row.environment_name, eb::state::STATUS_READY).await;
        let cname = ok_in(
            &emulator,
            case,
            row.region,
            "DescribeEnvironments",
            &[("EnvironmentIds.member.1", environment_id.as_str())],
        )
        .await
        .expect("Environments/member/CNAME", case)
        .to_string();

        // The prefix the launch holds: the one it supplied, or the environment's
        // own name with its unique id body appended for a launch that supplied
        // none — and the LABEL that prefix forms in the published address, which is
        // that prefix as one DNS label, since the published value is the authority
        // of an address a client dials.
        let prefix = row.cname_prefix.clone().unwrap_or_else(|| {
            format!(
                "{}-{}",
                row.environment_name,
                environment_id.trim_start_matches("e-")
            )
        });
        let label = service_common::data_plane_addressing::dns_label(&prefix);

        // The FORM, as the shared assertion spells it: the prefix's label first,
        // then this environment's own sub-domain, the Region, and this service's
        // domain in the zone the emulator announces its addresses in. It answers
        // the sub-domain label, which only the service can mint.
        let sub_domain =
            assert_published_cname(case, &cname, row.region, &prefix).to_string();

        // And the properties a client reads it for, asserted on the value itself
        // rather than on the form the assertion composed: a DNS name, within the
        // member's modelled length, whose first label is the prefix and whose
        // Region label sits behind the sub-domain, with a domain of its own behind
        // them.
        assert_is_a_host(case, "the CNAME", &cname);
        assert!(
            DNS_CNAME_LENGTH.contains(&cname.len()),
            "case {case:?}: expected a CNAME within the modelled {DNS_CNAME_LENGTH:?}, got {} in \
             {cname:?}",
            cname.len()
        );
        let labels: Vec<&str> = cname.split('.').collect();
        assert_eq!(
            labels.first().copied(),
            Some(label.as_str()),
            "case {case:?}: the first label must be the environment's CNAME prefix, got {cname:?}"
        );
        assert_eq!(
            labels.get(1).copied(),
            Some(sub_domain.as_str()),
            "case {case:?}: the second label must be the environment's own sub-domain, got \
             {cname:?}"
        );
        assert_eq!(
            labels.get(2).copied(),
            Some(row.region),
            "case {case:?}: the Region must be the label behind the sub-domain, got {cname:?}"
        );
        assert!(
            labels.len() >= 5 && labels.iter().all(|label| !label.is_empty()),
            "case {case:?}: expected a domain of its own behind the leading labels, got \
             {cname:?}"
        );

        // The documented way back to the prefix — the first dot-label — is a
        // prefix `CheckDNSAvailability` accepts (at least the 4 characters
        // `DNSCnamePrefix` admits) and reports as taken, because this environment
        // holds it.
        let recovered = labels
            .first()
            .copied()
            .unwrap_or_else(|| panic!("case {case:?}: expected a first label in {cname:?}"));
        assert_eq!(
            recovered, label,
            "case {case:?}: the first label must be the environment's CNAME prefix, got {cname:?}"
        );
        assert!(
            recovered.chars().count() >= 4 && eb::state::is_legal_cname_prefix(recovered),
            "case {case:?}: the recovered prefix must be a usable CNAME prefix, got \
             {recovered:?}"
        );
        let checked = ok_in(
            &emulator,
            case,
            row.region,
            "CheckDNSAvailability",
            &[("CNAMEPrefix", recovered)],
        )
        .await;
        assert_eq!(
            checked.value("Available"),
            Some("false"),
            "case {case:?}: the prefix this environment holds must not be available, got body {}",
            checked.body
        );
        // A prefix that is not available reserves nothing, so the report carries
        // no name for it: the name belongs to this environment, which publishes
        // it as the `CNAME` above.
        assert!(
            checked.value("FullyQualifiedCNAME").is_none(),
            "case {case:?}: a prefix that is not available reserves no name, got body {}",
            checked.body
        );
        assert_eq!(
            cname,
            format!("{label}.{sub_domain}.{}.{PUBLISHED_DOMAIN}", row.region),
            "case {case:?}: the published address must name this prefix in its Region under \
             this environment's own sub-domain"
        );

        // The EndpointURL is the ADDRESS of what answers behind the name — "the
        // URL to the LoadBalancer" for an environment with one, "the IP address of
        // the instance" for one without — and never the name itself.
        let endpoint = launched.expect("EndpointURL", case).to_string();
        assert_is_a_host(case, "the EndpointURL", &endpoint);
        assert_ne!(
            endpoint, cname,
            "case {case:?}: the EndpointURL is the address behind the name, not the name"
        );
        let load_balancer = load_balancer_of(&emulator, case, row.region, row.environment_name).await;
        let served_at = match load_balancer.as_deref() {
            Some(load_balancer) => emulator.published_load_balancer_host(row.region, load_balancer),
            None => emulator.published_instance_address(),
        };
        assert_eq!(
            endpoint, served_at,
            "case {case:?}: the EndpointURL must be the address the environment is served at, got \
             body {}",
            launched.body
        );
        // A single-instance environment has no load balancer to publish, and an
        // environment that leaves the documented default in force has one: the
        // member follows what the environment IS.
        assert_eq!(
            load_balancer.is_none(),
            row.environment_type == Some(eb::state::ENVIRONMENT_TYPE_SINGLE_INSTANCE),
            "case {case:?}: only a single-instance environment has no load balancer, got \
             {load_balancer:?}"
        );

        // And a later read reports the identical pair: both are the environment's,
        // not the reading caller's.
        let described = ok_in(
            &emulator,
            case,
            row.region,
            "DescribeEnvironments",
            &[("EnvironmentIds.member.1", environment_id.as_str())],
        )
        .await;
        assert_eq!(
            described.values_at("Environments/member/CNAME"),
            vec![cname.as_str()],
            "case {case:?}: every read reports the same published name, got body {}",
            described.body
        );
        assert_eq!(
            described.values_at("Environments/member/EndpointURL"),
            vec![endpoint.as_str()],
            "case {case:?}: every read reports the same EndpointURL, got body {}",
            described.body
        );
    }
}

/// The two bundles the swap below serves, each a static site carrying its own
/// content: a bundle with no runnable entry point serves the file a request
/// addresses, which is what makes the two environments tell themselves apart.
const BLUE_DOCUMENT: &[u8] = b"<!doctype html><title>blue</title><p>blue answers\n";
/// See [`BLUE_DOCUMENT`].
const GREEN_DOCUMENT: &[u8] = b"<!doctype html><title>green</title><p>green answers\n";

/// Upload a bundle carrying `document` as its root document, create a version of
/// [`APPLICATION`] from it, and launch an environment deploying that version under
/// `prefix`.
async fn launched(
    emulator: &Emulator,
    case: &str,
    environment_name: &str,
    prefix: &str,
    version: &str,
    document: &[u8],
) {
    let bucket = format!("{BUNDLE_BUCKET_PREFIX}-{version}");
    let key = "bundles/app.zip";
    emulator
        .put_object(&bucket, key, &zip_of(&[("index.html", document)]))
        .await;
    ok(
        emulator,
        case,
        "CreateApplicationVersion",
        &[
            ("ApplicationName", APPLICATION),
            ("VersionLabel", version),
            ("SourceBundle.S3Bucket", &bucket),
            ("SourceBundle.S3Key", key),
        ],
    )
    .await;
    ok(
        emulator,
        case,
        "CreateEnvironment",
        &[
            ("ApplicationName", APPLICATION),
            ("EnvironmentName", environment_name),
            ("CNAMEPrefix", prefix),
            ("SolutionStackName", SOLUTION_STACK.as_str()),
            ("VersionLabel", version),
        ],
    )
    .await;
}

/// The body the published host `host` serves for `/`, addressed exactly as the API
/// reported it — the address a client was given, with no credential of any kind.
async fn served(emulator: &Emulator, case: &str, host: &str) -> String {
    let answer = emulator.get_url(&format!("http://{host}/")).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected http://{host}/ to be served by the deployed application, got \
         {} and body {}",
        answer.status, answer.body
    );
    answer.body
}

/// The port the environment plane's socket answers on, which is what resolves a
/// published name: the name carries no port, so a client that resolved it dials
/// the socket this emulator serves the plane on.
fn plane_port(emulator: &Emulator, case: &str) -> u16 {
    let authority = emulator.plane_authority();
    authority
        .rsplit_once(':')
        .and_then(|(_host, port)| port.parse().ok())
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the plane to answer on a port, got {authority:?}")
        })
}

/// The same question asked over a REAL connection to the socket the plane is
/// served on: the whole HTTP/1.1 answer a client gets for `GET /` with the
/// published host `host` in the request's `Host` and nothing else naming the
/// environment.
///
/// This is the client the published name exists for, and it exercises the relay the
/// socket performs — a request whose addressing the front door must read back
/// exactly as the client stated it. The connection is opened to the LOOPBACK
/// literal, the interface the core binds its listeners on and the one a resolver
/// answers a name under the loopback zone with (RFC 6761), so what is under test is
/// the addressing and not the host's own resolver — the published name carries no
/// port, so the port is the plane socket's own.
///
/// `pub(crate)` because a sibling suite in this same binary asks the same question
/// of the same published addresses (`published_address_hosts.rs`): one copy of the
/// dial, so the two cannot drift about how a published address is reached.
pub(crate) async fn dialled(emulator: &Emulator, case: &str, host: &str) -> String {
    dialled_request(case, plane_port(emulator, case), host, "/").await
}

/// One request written on a fresh connection to the plane socket at `port`,
/// stating `host` and asking for `path`, and the whole answer it was given.
async fn dialled_request(case: &str, port: u16, host: &str, path: &str) -> String {
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nAccept: */*\r\nConnection: close\r\n\r\n");
    let mut connection = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap_or_else(|error| {
            panic!("case {case:?}: expected {host} to be dialable, got {error}")
        });
    tokio::io::AsyncWriteExt::write_all(&mut connection, request.as_bytes())
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: the request must be sent, got {error}"));
    let mut answered = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut connection, &mut answered)
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: the answer must be read, got {error}"));
    String::from_utf8_lossy(&answered).into_owned()
}

/// The published CNAME of the environment `environment_name`, read back through
/// `DescribeEnvironments`.
async fn published(emulator: &Emulator, case: &str, environment_name: &str) -> String {
    read_back(emulator, case, environment_name, "CNAME").await
}

/// The published `EndpointURL` of that environment, read back the same way.
async fn published_endpoint(emulator: &Emulator, case: &str, environment_name: &str) -> String {
    read_back(emulator, case, environment_name, "EndpointURL").await
}

/// One member of the environment `environment_name`, read back through
/// `DescribeEnvironments`.
async fn read_back(
    emulator: &Emulator,
    case: &str,
    environment_name: &str,
    member: &str,
) -> String {
    let described = ok(
        emulator,
        case,
        "DescribeEnvironments",
        &[("EnvironmentNames.member.1", environment_name)],
    )
    .await;
    described
        .expect(&format!("Environments/member/{member}"), case)
        .to_string()
}

/// RULE: the published name is a host this emulator ANSWERS at — a plain HTTP
/// request carrying it is served by the application of the environment that
/// carries that name — and `SwapEnvironmentCNAMEs` exchanges the two names, so each
/// name is then served by the OTHER environment's application while each
/// environment goes on reporting the name it now holds.
#[tokio::test]
async fn the_published_name_is_served_and_follows_a_swap() {
    let case = "a blue/green swap";
    let emulator = Emulator::with_object_storage();
    ok(
        &emulator,
        case,
        "CreateApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;
    launched(&emulator, case, "blue-env", "blue-site", "v-blue", BLUE_DOCUMENT).await;
    launched(
        &emulator,
        case,
        "green-env",
        "green-site",
        "v-green",
        GREEN_DOCUMENT,
    )
    .await;
    // "Ready: Environment is available to have an action performed on it, such as
    // update or terminate" — waited for as a CONDITION, never as a fixed time.
    for environment_name in ["blue-env", "green-env"] {
        wait_for_status(&emulator, case, environment_name, eb::state::STATUS_READY).await;
    }

    let blue = published(&emulator, case, "blue-env").await;
    let green = published(&emulator, case, "green-env").await;
    let blue_endpoint = published_endpoint(&emulator, case, "blue-env").await;
    assert_ne!(
        blue, green,
        "case {case:?}: the two environments must publish different names"
    );
    let blue_body = String::from_utf8_lossy(BLUE_DOCUMENT).to_string();
    let green_body = String::from_utf8_lossy(GREEN_DOCUMENT).to_string();
    assert_eq!(
        served(&emulator, case, &blue).await,
        blue_body,
        "case {case:?}: the name blue-env publishes must serve its own application"
    );
    assert_eq!(
        served(&emulator, case, &green).await,
        green_body,
        "case {case:?}: the name green-env publishes must serve its own application"
    );
    // And the address it is SERVED at — "the URL to the LoadBalancer", "the IP
    // address of the instance" — answers for its own environment too.
    assert_eq!(
        served(&emulator, case, &blue_endpoint).await,
        blue_body,
        "case {case:?}: the address blue-env is served at must serve its own application"
    );
    // The same, over a REAL connection to the socket the plane is served on,
    // which is the client the published name exists for.
    let answered = dialled(&emulator, case, &blue).await;
    assert!(
        answered.starts_with("HTTP/1.1 200") && answered.contains(&blue_body),
        "case {case:?}: a client dialling the name blue-env publishes must be served its own \
         application, got {answered:?}"
    );

    ok(
        &emulator,
        case,
        "SwapEnvironmentCNAMEs",
        &[
            ("SourceEnvironmentName", "blue-env"),
            ("DestinationEnvironmentName", "green-env"),
        ],
    )
    .await;

    // Each environment now reports the name the other held …
    assert_eq!(
        published(&emulator, case, "blue-env").await,
        green,
        "case {case:?}: the swap must give blue-env the name green-env held"
    );
    assert_eq!(
        published(&emulator, case, "green-env").await,
        blue,
        "case {case:?}: the swap must give green-env the name blue-env held"
    );
    // … and each NAME is served by the application of the environment that now
    // carries it, which is the whole consequence of a swap.
    assert_eq!(
        served(&emulator, case, &blue).await,
        green_body,
        "case {case:?}: after the swap the name blue-env held must serve green-env's application"
    );
    assert_eq!(
        served(&emulator, case, &green).await,
        blue_body,
        "case {case:?}: after the swap the name green-env held must serve blue-env's application"
    );
    // Only the NAMES moved: the address each environment is served at is its own,
    // exactly as a load balancer stays with the environment it fronts.
    assert_eq!(
        published_endpoint(&emulator, case, "blue-env").await,
        blue_endpoint,
        "case {case:?}: the swap must leave blue-env's EndpointURL where it was"
    );
    assert_eq!(
        served(&emulator, case, &blue_endpoint).await,
        blue_body,
        "case {case:?}: after the swap blue-env's own address must still serve its own \
         application"
    );
    // Both consequences again over a REAL connection, where the addressing the
    // client stated has to survive the socket's relay to the front door: the NAME
    // blue-env held is now green-env's application, and blue-env's own address is
    // still blue-env's.
    let answered = dialled(&emulator, case, &blue).await;
    assert!(
        answered.starts_with("HTTP/1.1 200") && answered.contains(&green_body),
        "case {case:?}: a client dialling the name blue-env held must be served green-env's \
         application after the swap, got {answered:?}"
    );
    let answered = dialled(&emulator, case, &green).await;
    assert!(
        answered.starts_with("HTTP/1.1 200") && answered.contains(&blue_body),
        "case {case:?}: a client dialling the name green-env held must be served blue-env's \
         application after the swap, got {answered:?}"
    );
    let answered = dialled(&emulator, case, &blue_endpoint).await;
    assert!(
        answered.starts_with("HTTP/1.1 200") && answered.contains(&blue_body),
        "case {case:?}: a client dialling blue-env's own address must be served blue-env's \
         application, got {answered:?}"
    );
}

/// RULE: which environment the socket an environment's address is served on
/// relays a request to is decided by the environments this service HOLDS, never
/// by the shape of the host a client wrote — so a host no environment of this
/// store answers at is served nothing, and a request whose PATH names a live
/// environment is served that environment's application whatever host it stated.
#[tokio::test]
async fn an_invented_host_decides_nothing_the_socket_relays() {
    let case = "an invented host";
    let emulator = Emulator::with_object_storage();
    ok(
        &emulator,
        case,
        "CreateApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;
    launched(&emulator, case, "blue-env", "blue-site", "v-blue", BLUE_DOCUMENT).await;
    wait_for_status(&emulator, case, "blue-env", eb::state::STATUS_READY).await;

    let port = plane_port(&emulator, case);
    // A host shaped exactly like one of this service's published names — down to
    // a first label naming another service — that no environment answers at.
    let invented = format!("s3.us-east-1.elasticbeanstalk.localhost:{port}");
    let blue_path = eb::state::environment_plane_path(TEST_REGION, "blue-site");
    let blue_body = String::from_utf8_lossy(BLUE_DOCUMENT).to_string();

    let rows: &[(&str, &str, &str, Option<&str>)] = &[
        (
            "a name no environment of this store answers at",
            invented.as_str(),
            "/",
            None,
        ),
        (
            "that same host, with the plane path of a live environment",
            invented.as_str(),
            blue_path.as_str(),
            Some(blue_body.as_str()),
        ),
        (
            "the loopback address this socket answers on, with that path",
            "127.0.0.1",
            blue_path.as_str(),
            Some(blue_body.as_str()),
        ),
        (
            "a load balancer no environment of this store holds",
            "awseb-e-nothing-stack-awsebloadbalancer-xyz.us-west-2.elb.localhost",
            "/",
            None,
        ),
    ];
    for (row, host, path, expected) in rows {
        let answered = dialled_request(row, port, host, path).await;
        match expected {
            Some(body) => assert!(
                answered.starts_with("HTTP/1.1 200") && answered.contains(body),
                "case {row:?}: expected the application named by the path, got {answered:?}"
            ),
            None => assert!(
                answered.starts_with("HTTP/1.1 404"),
                "case {row:?}: expected nothing to be served, got {answered:?}"
            ),
        }
    }
}
