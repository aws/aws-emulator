// SPDX-License-Identifier: CC0-1.0

//! Whose HOST an environment's published address sits on.
//!
//! RULE: every value this service publishes as an address for a client to DIAL —
//! `EnvironmentDescription$CNAME` ("the URL to the CNAME for this environment")
//! and the `EndpointURL` beside it ("the URL to the LoadBalancer", "the IP address
//! of the instance") — is a HOST this emulator answers at: a name in the zone it
//! may announce a name of its own in (the loopback zone RFC 6761 reserves for the
//! interface the core binds its listeners on), or a loopback address literal. It
//! is never a name in Elastic Beanstalk's public DNS zone or any other AWS
//! production domain, which resolve to nothing on the machine this emulator runs
//! on: the name that zone forms is a different value, reported where the service
//! reports on a NAME rather than on an address
//! (`CheckDNSAvailability$FullyQualifiedCNAME`), so the address a client dials and
//! that name are never the same string. Neither value carries a port or a path —
//! a host carries neither, and the port and the path a request needs are resolved
//! at the listener the plane is served on.
//!
//! `published_environment_name.rs` holds the published CNAME to its FORM — the
//! prefix label, the environment's own sub-domain, the Region and this service's
//! own domain, over the prefix lengths and cases `DNSCnamePrefix` admits — so this
//! suite does not repeat that table. What it spans is what THIS rule adds: which
//! operation minted the address (`CreateEnvironment` here, `ComposeEnvironments`
//! in the second suite), whether the launch supplied the CNAME prefix or left it
//! to be generated, the Region, whether an application is deployed behind the
//! address, and whether the value is read from the launch's own answer or from a
//! later `DescribeEnvironments`. Every published address is then dialled and has to
//! be ANSWERED by this emulator, which an address in a public AWS zone could not
//! be.
//!
//! The HTTP harness (the router, the form encoder, the XML element reader, the
//! object-storage upload and the poll for a settled status) is the one
//! `lifecycle.rs` owns in this same test binary; the archive writer is
//! `application_version_processing.rs`'s, the manifest writer and the composition
//! helpers are `composed_environments.rs`'s, and the real connection to a
//! published address is `published_environment_name.rs`'s `dialled`.

use crate::application_version_processing::zip_of;
use crate::composed_environments::{compose, given_application, given_version, manifest};
use crate::harness::{wait_for_status, Emulator, SOLUTION_STACK};
use crate::lifecycle::TEST_REGION;
use crate::published_environment_name::{dialled, ok_in};

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

/// The application the launches in the first suite are made under.
const APPLICATION: &str = "published-address-app";

/// The bucket each source bundle is uploaded into, one per version label.
const BUNDLE_BUCKET_PREFIX: &str = "eb-published-address-bundles";

/// The key every source bundle is uploaded under in its own bucket.
const BUNDLE_KEY: &str = "bundles/app.zip";

/// The document the deployed row serves as its root document: a bundle with no
/// runnable entry point serves the file a request addresses, so a client dialling
/// the published address reads this back.
const LAUNCH_DOCUMENT: &[u8] = b"<!doctype html><title>launched</title><p>the launch answers\n";

/// The documents the two composed environments of the second suite serve, so each
/// published address is asserted to reach its OWN environment's application.
const FRONT_DOCUMENT: &[u8] = b"<!doctype html><title>front</title><p>front answers\n";
/// See [`FRONT_DOCUMENT`].
const WORKER_DOCUMENT: &[u8] = b"<!doctype html><title>worker</title><p>worker answers\n";

/// The one host name a client resolves without a resolver of its own, which is
/// the only name a published address may carry in place of a loopback literal.
const LOOPBACK_NAME: &str = "localhost";

/// The domain of the hosts an AWS production endpoint publishes, which no address
/// this emulator hands a client may sit under.
const PRODUCTION_DOMAIN: &str = "amazonaws.com";

/// The zone a CNAME prefix forms its fully qualified DNS NAME in, as Elastic
/// Beanstalk publishes it (`CheckDNSAvailability$FullyQualifiedCNAME`, "the fully
/// qualified CNAME to reserve when CreateEnvironment is called with the provided
/// prefix") — spelled out here rather than read off the crate, so the expectation
/// holds the service to the published form.
const PUBLISHED_CNAME_ZONE: &str = "elasticbeanstalk.com";

/// The status line this emulator answers a client at an address it serves for an
/// environment running no application: the status a proxy answers for a request
/// it has no upstream for, which is where this emulator's environment plane
/// renders that case (`src/service/environment_plane.rs`,
/// `NO_APPLICATION_STATUS`). An answer from this emulator, which is what the rule
/// is about, and not the one an address serving no environment of this service at
/// all would get.
const NO_APPLICATION_STATUS_LINE: &str = "HTTP/1.1 502";

/// One launch, and what a client dialling the address it publishes must then be
/// served.
struct AddressRow {
    case: &'static str,
    /// The Region the launch is signed for: the same prefix in two Regions is two
    /// addresses, so a launch elsewhere publishes another address on this same
    /// host.
    region: &'static str,
    environment_name: &'static str,
    /// The `CNAMEPrefix` the launch supplies, or `None` for a launch that leaves
    /// it to be generated ("if not specified, the CNAME is generated
    /// automatically by appending a random alphanumeric string to the environment
    /// name").
    cname_prefix: Option<&'static str>,
    /// The version label whose bundle this launch deploys, for the row that puts
    /// an application behind the address; `None` for a launch that deploys none.
    version_label: Option<&'static str>,
}

/// The rows, one per form this rule generalises over.
fn rows() -> Vec<AddressRow> {
    vec![
        AddressRow {
            case: "a supplied prefix, with an application deployed",
            region: TEST_REGION,
            environment_name: "addr-deployed",
            cname_prefix: Some("addr-deployed-site"),
            version_label: Some("v-addr"),
        },
        AddressRow {
            case: "a generated prefix",
            region: TEST_REGION,
            environment_name: "addr-generated",
            cname_prefix: None,
            version_label: None,
        },
        AddressRow {
            case: "another Region",
            region: "eu-central-1",
            environment_name: "addr-other-region",
            cname_prefix: Some("addr-elsewhere"),
            version_label: None,
        },
    ]
}

/// Hold one published address to this rule: a HOST this emulator answers at, and
/// nothing of an AWS production address.
///
/// What is asserted is the address itself: no part of it in Elastic Beanstalk's
/// public DNS zone or an AWS production domain, no port separator and no path
/// separator — a host carries neither — and a host a client of this emulator
/// reaches, which is a loopback address literal or a name under the zone RFC 6761
/// reserves for the loopback interface the core binds its listeners on. The FORM of
/// the labels in front of that zone is `published_environment_name.rs`'s subject,
/// not this one's.
fn emulator_address(case: &str, member: &str, address: &str) {
    assert!(
        !address.contains(PUBLISHED_CNAME_ZONE) && !address.contains(PRODUCTION_DOMAIN),
        "case {case:?}: the published {member} must not be an AWS production address, got \
         {address:?}"
    );
    assert!(
        !address.contains(':') && !address.contains('/'),
        "case {case:?}: the published {member} is a host, carrying neither ':' nor '/', got \
         {address:?}"
    );
    assert!(
        address.split('.').all(|label| !label.is_empty()),
        "case {case:?}: the published {member} must carry no empty label, got {address:?}"
    );
    let on_this_machine = address
        .parse::<std::net::IpAddr>()
        .map_or(address == LOOPBACK_NAME || address.ends_with(&format!(".{LOOPBACK_NAME}")), |ip| {
            ip.is_loopback()
        });
    assert!(
        on_this_machine,
        "case {case:?}: the published {member} must be a host this emulator answers at, got \
         {address:?}"
    );
}

/// The whole HTTP/1.1 answer a client gets for `GET /` of the published address
/// `address`, named for the `member` that published it.
///
/// One dial for the whole target: `published_environment_name::dialled` opens the
/// connection to the loopback literal on the port the environment plane answers at
/// — the published host carries none — and states that host in the request's
/// `Host`, so what is under test is the addressing rather than the host's own
/// resolver; [`emulator_address`] is what holds the published host itself to the
/// loopback the core binds.
async fn answer_at(emulator: &Emulator, case: &str, member: &str, address: &str) -> String {
    dialled(emulator, &format!("{case}, the published {member}"), address).await
}

/// Upload a bundle carrying [`LAUNCH_DOCUMENT`] as its root document and create
/// the application version `version_label` of [`APPLICATION`] from it.
async fn given_bundle(emulator: &Emulator, case: &str, version_label: &str) {
    let bucket = format!("{BUNDLE_BUCKET_PREFIX}-{version_label}");
    emulator
        .put_object(
            &bucket,
            BUNDLE_KEY,
            &zip_of(&[("index.html", LAUNCH_DOCUMENT)]),
        )
        .await;
    ok_in(
        emulator,
        case,
        TEST_REGION,
        "CreateApplicationVersion",
        &[
            ("ApplicationName", APPLICATION),
            ("VersionLabel", version_label),
            ("SourceBundle.S3Bucket", &bucket),
            ("SourceBundle.S3Key", BUNDLE_KEY),
        ],
    )
    .await;
}

/// RULE: the address an environment publishes sits on a host this emulator
/// answers on and is never a name in an AWS production domain — so a client is
/// answered when it dials it, while the name Elastic Beanstalk's public zone
/// forms for the same prefix stays where the API reports on a NAME.
#[tokio::test]
async fn no_published_environment_address_is_an_aws_production_name() {
    let emulator = Emulator::with_object_storage();
    ok_in(
        &emulator,
        "setup",
        TEST_REGION,
        "CreateApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;

    for row in rows() {
        let case = row.case;
        if let Some(version_label) = row.version_label {
            given_bundle(&emulator, case, version_label).await;
        }
        let mut params: Vec<(&str, &str)> = vec![
            ("ApplicationName", APPLICATION),
            ("EnvironmentName", row.environment_name),
            ("SolutionStackName", SOLUTION_STACK.as_str()),
        ];
        if let Some(prefix) = row.cname_prefix {
            params.push(("CNAMEPrefix", prefix));
        }
        if let Some(version_label) = row.version_label {
            params.push(("VersionLabel", version_label));
        }
        let launched = ok_in(&emulator, case, row.region, "CreateEnvironment", &params).await;
        let environment_id = launched.expect("EnvironmentId", case).to_string();
        // The launch reports the address of what answers BEHIND the name from the
        // start; the NAME itself is published once the provisioning that assigns
        // it has finished, so it is read off the environment below.
        assert!(
            launched.value("CNAME").is_none(),
            "case {case:?}: a launch still provisioning reports no CNAME, got {}",
            launched.body
        );
        let endpoint_url = launched.expect("EndpointURL", case).to_string();
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

        // Both published addresses obey the rule …
        emulator_address(case, "CNAME", &cname);
        emulator_address(case, "EndpointURL", &endpoint_url);
        // … and they are different values: the NAME the environment answers at,
        // and the address of what answers behind it.
        assert_ne!(
            cname, endpoint_url,
            "case {case:?}: the EndpointURL is the address behind the name, not the name"
        );

        // The prefix the NAME carries — its first dot-label, the documented route
        // back to it — is what the PUBLIC DNS name is formed from, and the two
        // values are never the same string: one is an address a client dials, the
        // other the name the service reports ON. The environment HOLDS that prefix,
        // so the report says the name is not available and reserves none for this
        // caller; the public name it forms is built here from the same two parts
        // the service builds it from.
        let prefix = cname.split('.').next().unwrap_or_default().to_string();
        let checked = ok_in(
            &emulator,
            case,
            row.region,
            "CheckDNSAvailability",
            &[("CNAMEPrefix", prefix.as_str())],
        )
        .await;
        assert_eq!(
            checked.expect("Available", case),
            "false",
            "case {case:?}: the launched environment holds that prefix, got body {}",
            checked.body
        );
        let reported_name = format!("{prefix}.{}.{PUBLISHED_CNAME_ZONE}", row.region);
        assert_ne!(
            cname, reported_name,
            "case {case:?}: the address a client dials must not be the public DNS name the prefix \
             forms"
        );

        // A later read publishes addresses the rule holds for too: the rule is
        // about every address this service hands a client, not only the one the
        // launch answered with.
        let described = ok_in(
            &emulator,
            case,
            row.region,
            "DescribeEnvironments",
            &[("EnvironmentIds.member.1", environment_id.as_str())],
        )
        .await;
        for member in ["CNAME", "EndpointURL"] {
            let read_back = described
                .expect(&format!("Environments/member/{member}"), case)
                .to_string();
            emulator_address(case, member, &read_back);
        }

        // And the client the address exists for: a request stating the published
        // host, on the port the plane answers at. The application behind the
        // address is deployed by the launch, which has finished above — waited
        // for as a CONDITION, never as a fixed time.
        let expected = row
            .version_label
            .map(|_| String::from_utf8_lossy(LAUNCH_DOCUMENT).to_string());
        for (member, address) in [("CNAME", &cname), ("EndpointURL", &endpoint_url)] {
            let answered = answer_at(&emulator, case, member, address).await;
            match expected.as_deref() {
                Some(document) => assert!(
                    answered.starts_with("HTTP/1.1 200") && answered.contains(document),
                    "case {case:?}: a client dialling the published {member} must be served the \
                     deployed application, got {answered:?}"
                ),
                // Nothing is deployed behind this address, so what answers is
                // this emulator's own answer for an address it SERVES whose
                // environment runs no application — not the status an address
                // serving no environment of this service would get.
                None => assert!(
                    answered.starts_with(NO_APPLICATION_STATUS_LINE),
                    "case {case:?}: a client dialling the published {member} must be answered by \
                     this emulator for an address it serves, got {answered:?}"
                ),
            }
        }
    }
}

/// RULE through the OTHER launch: a `ComposeEnvironments` launch publishes the
/// same kind of address — on this emulator's own listener, never in an AWS
/// production domain — and each composed environment's address is served by the
/// application that environment runs.
#[tokio::test]
async fn a_composed_environment_publishes_an_address_this_emulator_serves() {
    let case = "a composed group";
    let emulator = Emulator::with_object_storage();
    given_application(&emulator, case).await;
    let stack = SOLUTION_STACK.as_str().to_string();
    for (label, environment_name, document) in [
        ("addr-front", "addr-front-env", FRONT_DOCUMENT),
        ("addr-worker", "addr-worker-env", WORKER_DOCUMENT),
    ] {
        given_version(
            &emulator,
            case,
            label,
            &[
                (
                    "env.yaml",
                    &manifest(environment_name, Some(&stack), None, &[], &[]),
                ),
                ("index.html", document),
            ],
        )
        .await;
    }

    let composed = compose(&emulator, None, &["addr-front", "addr-worker"]).await;
    assert_eq!(
        composed.status, 200,
        "case {case:?}: expected 200 from ComposeEnvironments, got {} and body {}",
        composed.status, composed.body
    );
    let environments = composed.elements_at("Environments/member");
    assert_eq!(
        environments.len(),
        2,
        "case {case:?}: expected one environment per version label, got body {}",
        composed.body
    );

    for (element, (environment_name, document)) in environments.iter().zip([
        ("addr-front-env", FRONT_DOCUMENT),
        ("addr-worker-env", WORKER_DOCUMENT),
    ]) {
        let case = &format!("{case}: {environment_name}");
        assert_eq!(
            element.value("EnvironmentName"),
            Some(environment_name),
            "case {case:?}: the composed environments must be reported in the order composed, got \
             body {}",
            composed.body
        );
        // The composition reports the address of what answers behind the name
        // from the start; the NAME is published once the launch has finished.
        assert!(
            element.value("CNAME").is_none(),
            "case {case:?}: a launch still provisioning reports no CNAME, got body {}",
            composed.body
        );
        let endpoint_url = element.expect("EndpointURL", case).to_string();
        emulator_address(case, "EndpointURL", &endpoint_url);

        wait_for_status(&emulator, case, environment_name, eb::state::STATUS_READY).await;
        let cname = ok_in(
            &emulator,
            case,
            TEST_REGION,
            "DescribeEnvironments",
            &[("EnvironmentNames.member.1", environment_name)],
        )
        .await
        .expect("Environments/member/CNAME", case)
        .to_string();
        emulator_address(case, "CNAME", &cname);
        let document = String::from_utf8_lossy(document).to_string();
        for (member, address) in [("CNAME", &cname), ("EndpointURL", &endpoint_url)] {
            let answered = answer_at(&emulator, case, member, address).await;
            assert!(
                answered.starts_with("HTTP/1.1 200") && answered.contains(&document),
                "case {case:?}: a client dialling the published {member} must be served this \
                 environment's own application, got {answered:?}"
            );
        }
    }
}
