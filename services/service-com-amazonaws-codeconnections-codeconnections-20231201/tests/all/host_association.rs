// SPDX-License-Identifier: CC0-1.0

//! Hosts, and a connection's association with one.
//!
//! `CreateHost` mints and PERSISTS a host, `GetHost` reports that stored row back, and
//! `CreateConnection` RESOLVES its optional `HostArn` against the host store: a host the
//! store holds is associated with the new connection and reported by every reader, and
//! only an ARN naming no host is refused.
//!
//! Every request goes through the real `MultiServiceRouter` dispatch, reusing the
//! `connection_lifecycle` suite's helpers (`router`, `call`, `create`), so these tests
//! exercise the same path a signed SDK request takes.

use std::collections::BTreeSet;

use smithy_server_runtime::MultiServiceRouter;

use super::connection_lifecycle::{PROVIDER_TYPES, REGION, call, create, router};

/// A `VpcConfiguration` list is `length 1..10` on both `SubnetIds` and
/// `SecurityGroupIds` (`smithy_query(query="shape", shape="SubnetIds,SecurityGroupIds")`),
/// so the table below covers each published bound.
pub(crate) const VPC_LIST_MAX: usize = 10;

/// One `CreateHost` request whose round trip this suite asserts.
///
/// `pub(crate)` because the host table below is the set of host shapes every host
/// suite needs: `list_hosts.rs` reports the same rows back rather than building a
/// second table of its own.
pub(crate) struct HostCase {
    /// What a failing assertion names this row by.
    pub(crate) label: &'static str,
    pub(crate) name: String,
    pub(crate) provider_type: &'static str,
    pub(crate) provider_endpoint: String,
    /// The `VpcConfiguration` the create carries, or `None` for a host without one.
    pub(crate) vpc_configuration: Option<serde_json::Value>,
    /// Tags as `(Key, Value)`, in the order the create supplies them.
    pub(crate) tags: Vec<(&'static str, &'static str)>,
}

/// `subnet-<8 or 17 word characters>` — the modeled `SubnetId` pattern
/// `^subnet-\w{8}(\w{9})?$`.
fn subnet_id(index: usize) -> String {
    format!("subnet-{index:08x}")
}

/// `sg-<8 or 17 word characters>` — the modeled `SecurityGroupId` pattern
/// `^sg-\w{8}(\w{9})?$`.
fn security_group_id(index: usize) -> String {
    format!("sg-{index:08x}")
}

/// A `VpcConfiguration` with `subnets` subnet ids and `groups` security group ids,
/// optionally carrying a TLS certificate.
///
/// `pub(crate)` because the VPC shapes a host carries are the same set every host suite
/// needs: `host_update.rs` sends these as the REPLACEMENT configuration rather than
/// building a second builder of its own.
pub(crate) fn vpc_configuration(
    subnets: usize,
    groups: usize,
    tls: Option<&str>,
) -> serde_json::Value {
    let mut config = serde_json::json!({
        // `VpcId` is `length 12..21`, pattern `^vpc-\w{8}(\w{9})?$`.
        "VpcId": "vpc-0123abcd",
        "SubnetIds": (0..subnets).map(subnet_id).collect::<Vec<_>>(),
        "SecurityGroupIds": (0..groups).map(security_group_id).collect::<Vec<_>>(),
    });
    if let Some(certificate) = tls {
        config["TlsCertificate"] = serde_json::Value::String(certificate.to_string());
    }
    config
}

/// The table: hosts differing in provider type, endpoint form and length, whether a
/// VPC configuration is present and how large its lists are (each published bound),
/// and how many tags they carry.
pub(crate) fn host_cases() -> Vec<HostCase> {
    vec![
        HostCase {
            label: "no vpc, no tags",
            name: "ghe-plain".to_string(),
            provider_type: "GitHubEnterpriseServer",
            provider_endpoint: "https://ghe.example.internal".to_string(),
            vpc_configuration: None,
            tags: Vec::new(),
        },
        HostCase {
            label: "vpc at the published list minimum, one tag",
            name: "gitlab-self-managed".to_string(),
            provider_type: "GitLabSelfManaged",
            provider_endpoint: "https://gitlab.example.internal:8443/path".to_string(),
            vpc_configuration: Some(vpc_configuration(1, 1, None)),
            tags: vec![("team", "platform")],
        },
        HostCase {
            label: "vpc at the published list maximum with a TLS certificate, two tags",
            name: "bitbucket-dc".to_string(),
            provider_type: "Bitbucket",
            provider_endpoint: "http://10.0.0.7".to_string(),
            vpc_configuration: Some(vpc_configuration(
                VPC_LIST_MAX,
                VPC_LIST_MAX,
                Some("-----BEGIN CERTIFICATE-----\nZm9v\n-----END CERTIFICATE-----"),
            )),
            tags: vec![("team", "platform"), ("env", "")],
        },
        HostCase {
            // `HostName` is `length 1..64` and `Url` is `length 1..512`.
            label: "name and endpoint at their published minimum length",
            name: "h".to_string(),
            provider_type: "AzureDevOps",
            provider_endpoint: "x".to_string(),
            vpc_configuration: None,
            tags: vec![("only", "one")],
        },
        HostCase {
            label: "name and endpoint at their published maximum length",
            name: "n".repeat(64),
            provider_type: "GitHub",
            provider_endpoint: format!("https://{}", "e".repeat(512 - "https://".len())),
            vpc_configuration: Some(vpc_configuration(2, 3, None)),
            tags: Vec::new(),
        },
        HostCase {
            label: "a provider type the issue did not cite",
            name: "gitlab-saas".to_string(),
            provider_type: "GitLab",
            provider_endpoint: "https://gitlab.com".to_string(),
            vpc_configuration: None,
            tags: vec![("a", "1"), ("b", "2"), ("c", "3")],
        },
    ]
}

/// Render a `(Key, Value)` table row as the wire `TagList`.
fn tag_list(tags: &[(&str, &str)]) -> serde_json::Value {
    serde_json::Value::Array(
        tags.iter()
            .map(|(key, value)| serde_json::json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// The `CreateHost` request body for one row.
fn create_host_body(case: &HostCase) -> serde_json::Value {
    let mut body = serde_json::json!({
        "Name": case.name,
        "ProviderType": case.provider_type,
        "ProviderEndpoint": case.provider_endpoint,
    });
    if let Some(vpc) = &case.vpc_configuration {
        body["VpcConfiguration"] = vpc.clone();
    }
    if !case.tags.is_empty() {
        body["Tags"] = tag_list(&case.tags);
    }
    body
}

/// Create a host and return its ARN, failing the test on any non-200.
pub(crate) async fn create_host(router: &MultiServiceRouter, case: &HostCase) -> String {
    let (status, body) = call(router, "CreateHost", create_host_body(case)).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected 200 from CreateHost, got {status} {body}",
        case.label
    );
    body["HostArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {:?}: CreateHost returned no HostArn: {body}", case.label))
        .to_string()
}

/// A host of the simplest shape, named `name`, for a test that only needs one to exist.
pub(crate) fn plain_host(name: &str, provider_type: &'static str) -> HostCase {
    HostCase {
        label: "plain host",
        name: name.to_string(),
        provider_type,
        provider_endpoint: format!("https://{name}.example.internal"),
        vpc_configuration: None,
        tags: Vec::new(),
    }
}

/// Create one plain host over the wire and return its ARN: what another suite needs
/// when it only needs a host to BE there (`absent_resource_answers.rs`).
pub(crate) async fn create_plain_host(
    router: &MultiServiceRouter,
    name: &str,
    provider_type: &'static str,
) -> String {
    create_host(router, &plain_host(name, provider_type)).await
}

/// The sentence this service answers for an ARN that resolves to no host, written out as
/// the LITERAL its host readers return rather than read off the implementation's own
/// constant.
///
/// `pub(crate)` and stated HERE, beside the host helpers, because every suite that
/// addresses a host by ARN asserts the same sentence (`host_deletion.rs`,
/// `host_update.rs`, `refusal_messages.rs`): one literal for the whole test target, so a
/// reworded constant fails all of them rather than one.
pub(crate) fn host_not_found_message(host_arn: &str) -> String {
    format!("No host exists with the ARN: {host_arn}")
}

/// Assert an ARN satisfies the modeled `HostArn` pattern
/// `^arn:aws(-[\w]+)*:(codestar-connections|codeconnections):.+:[0-9]{12}:host\/.+$`
/// and names a host in the region the caller signed for.
fn assert_host_arn(case: &str, arn: &str) -> String {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(
        fields.len(),
        6,
        "case {case:?}: expected a six-field ARN, got {arn:?}"
    );
    assert_eq!(fields[0], "arn", "case {case:?}: expected 'arn', got {arn:?}");
    assert_eq!(
        fields[1], "aws",
        "case {case:?}: expected partition 'aws', got {arn:?}"
    );
    assert_eq!(
        fields[2], "codeconnections",
        "case {case:?}: expected the codeconnections namespace, got {arn:?}"
    );
    assert_eq!(
        fields[3], REGION,
        "case {case:?}: expected the signed region, got {arn:?}"
    );
    assert_eq!(
        fields[4].len(),
        12,
        "case {case:?}: expected a 12-digit account id, got {arn:?}"
    );
    assert!(
        fields[4].bytes().all(|byte| byte.is_ascii_digit()),
        "case {case:?}: expected a 12-digit account id, got {arn:?}"
    );
    let resource = fields[5];
    assert!(
        resource.len() > "host/".len() && resource.starts_with("host/"),
        "case {case:?}: expected a 'host/<id>' resource, got {arn:?}"
    );
    assert!(
        arn.len() <= 256,
        "case {case:?}: a HostArn is at most 256 characters, got {} in {arn:?}",
        arn.len()
    );
    resource["host/".len()..].to_string()
}

/// A host round trips: every member `CreateHost` stored is what `GetHost` reports, the
/// host is `PENDING` as created, and its tags come back from `ListTagsForResource`.
#[tokio::test]
async fn a_host_round_trips_from_create_to_get() {
    let router = router();
    let mut arns = BTreeSet::new();

    for case in host_cases() {
        let arn = create_host(&router, &case).await;
        assert_host_arn(case.label, &arn);
        assert!(
            arns.insert(arn.clone()),
            "case {:?}: every host gets a distinct ARN, saw {arn:?} twice",
            case.label
        );

        let (status, got) = call(&router, "GetHost", serde_json::json!({ "HostArn": arn })).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetHost, got {status} {got}",
            case.label
        );
        assert_eq!(
            got["Name"],
            serde_json::json!(case.name),
            "case {:?}: expected the stored Name, got {got}",
            case.label
        );
        assert_eq!(
            got["ProviderType"],
            serde_json::json!(case.provider_type),
            "case {:?}: expected the stored ProviderType, got {got}",
            case.label
        );
        assert_eq!(
            got["ProviderEndpoint"],
            serde_json::json!(case.provider_endpoint),
            "case {:?}: expected the stored ProviderEndpoint, got {got}",
            case.label
        );
        // `CreateHost`: "A host created through the CLI or the SDK is in PENDING
        // status by default."
        assert_eq!(
            got["Status"],
            serde_json::json!("PENDING"),
            "case {:?}: expected a PENDING host, got {got}",
            case.label
        );
        match &case.vpc_configuration {
            Some(vpc) => assert_eq!(
                &got["VpcConfiguration"], vpc,
                "case {:?}: expected the stored VpcConfiguration, got {got}",
                case.label
            ),
            None => assert!(
                got["VpcConfiguration"].is_null(),
                "case {:?}: a host created with no VpcConfiguration reports none, got {got}",
                case.label
            ),
        }

        let (status, tags) = call(
            &router,
            "ListTagsForResource",
            serde_json::json!({ "ResourceArn": arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from ListTagsForResource, got {status} {tags}",
            case.label
        );
        assert_eq!(
            tags["Tags"],
            tag_list(&case.tags),
            "case {:?}: expected the tags the create supplied, got {tags}",
            case.label
        );
    }
}

/// `CreateHost` reports the created host's tags, and reports none when the host
/// carries none — the same rendering `CreateConnection` uses.
#[tokio::test]
async fn create_host_reports_the_tags_it_stored() {
    let router = router();
    for case in host_cases() {
        let (status, body) = call(&router, "CreateHost", create_host_body(&case)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from CreateHost, got {status} {body}",
            case.label
        );
        if case.tags.is_empty() {
            assert!(
                body["Tags"].is_null(),
                "case {:?}: a host created with no tags reports no tag list, got {body}",
                case.label
            );
        } else {
            assert_eq!(
                body["Tags"],
                tag_list(&case.tags),
                "case {:?}: expected the tags the create supplied, got {body}",
                case.label
            );
        }
    }
}

/// Every minted host resource id is opaque: 32 lowercase hex characters grouped
/// 8-4-4-4-12, distinct per host and derived from nothing in the request.
#[tokio::test]
async fn every_minted_host_resource_id_is_opaque_and_distinct() {
    let router = router();
    // The names differ only in their index, so an id derived from the name would show
    // up here as a shared prefix or an index in the id.
    const HOSTS: usize = 12;

    let mut ids = BTreeSet::new();
    for index in 0..HOSTS {
        let case = plain_host(&format!("host-{index}"), "GitHubEnterpriseServer");
        let arn = create_host(&router, &case).await;
        let id = assert_host_arn(&format!("host-{index}"), &arn);
        let groups: Vec<&str> = id.split('-').collect();
        assert_eq!(
            groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12],
            "case host-{index}: expected an 8-4-4-4-12 id, got {id:?}"
        );
        assert!(
            id.bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase() || byte == b'-'),
            "case host-{index}: expected lowercase hex, got {id:?}"
        );
        assert!(
            ids.insert(id.clone()),
            "case host-{index}: expected a distinct id, saw {id:?} twice"
        );
    }
}

/// A `HostArn` that resolves is stored on the connection and reported by every
/// reader; one row per modeled `ProviderType`, plus a connection with no host.
#[tokio::test]
async fn a_connection_resolves_a_host_that_exists() {
    let router = router();

    // One host per provider type, and one connection associated with each. The names are
    // built from the row's INDEX, not from the provider: the modelled `ConnectionName` is
    // `length 1..32`, which the longest provider value overruns.
    let mut by_host: Vec<(&str, String, String)> = Vec::new();
    for (index, provider) in PROVIDER_TYPES.iter().enumerate() {
        let host_arn = create_host(&router, &plain_host(&format!("host-{index}"), provider)).await;
        let (status, created) = call(
            &router,
            "CreateConnection",
            serde_json::json!({
                "ConnectionName": format!("conn-{index}"),
                "ProviderType": provider,
                "HostArn": host_arn,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {provider:?}: a HostArn that names a stored host must be accepted, got {status} {created}"
        );
        let connection_arn = created["ConnectionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {provider:?}: no ConnectionArn: {created}"))
            .to_string();
        by_host.push((provider, host_arn, connection_arn));
    }

    // A connection with no host: the member stays absent, and no host filter selects it.
    let hostless = create(&router, "no-host", "GitHub").await;

    for (provider, host_arn, connection_arn) in &by_host {
        let (status, got) = call(
            &router,
            "GetConnection",
            serde_json::json!({ "ConnectionArn": connection_arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {provider:?}: expected 200 from GetConnection, got {status} {got}"
        );
        assert_eq!(
            got["Connection"]["HostArn"],
            serde_json::json!(host_arn),
            "case {provider:?}: expected GetConnection to report the host, got {got}"
        );

        let (status, filtered) = call(
            &router,
            "ListConnections",
            serde_json::json!({ "HostArnFilter": host_arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {provider:?}: expected 200 from ListConnections, got {status} {filtered}"
        );
        let selected: Vec<&str> = filtered["Connections"]
            .as_array()
            .unwrap_or_else(|| panic!("case {provider:?}: Connections is not a list: {filtered}"))
            .iter()
            .map(|connection| {
                connection["ConnectionArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {provider:?}: no ConnectionArn: {filtered}"))
            })
            .collect();
        assert_eq!(
            selected,
            vec![connection_arn.as_str()],
            "case {provider:?}: HostArnFilter must select exactly that host's connections, got {selected:?}"
        );
        assert!(
            !selected.contains(&hostless.as_str()),
            "case {provider:?}: a connection with no host is selected by no host filter, got {selected:?}"
        );
    }

    let (status, got) = call(
        &router,
        "GetConnection",
        serde_json::json!({ "ConnectionArn": hostless }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case no-host: expected 200 from GetConnection, got {status} {got}"
    );
    assert!(
        got["Connection"]["HostArn"].is_null(),
        "case no-host: a connection created with no HostArn reports none, got {got}"
    );
}

/// Only an ARN naming no host is refused, and it is refused the same way wherever it
/// is named: the modeled `ResourceNotFoundException` from `CreateConnection`, from
/// `GetHost` and from `ListConnections`' filter, because the host it names is ABSENT —
/// `ResourceUnavailableException` is this service's "the host is there but is not
/// usable" signal. Both arrive with the client status of this service's RPC protocol
/// (400) and are told apart by the error shape named in the body. Rows are
/// host-ARN-shaped values the modeled pattern admits —
/// `^arn:aws(-[\w]+)*:(codestar-connections|codeconnections):.+:[0-9]{12}:host\/.+$` —
/// that this store does not hold.
#[tokio::test]
async fn a_host_arn_that_names_no_host_is_refused_wherever_it_is_named() {
    let router = router();
    let account = engine::emulator_account_id();
    // A host that IS there, so "unresolvable" cannot mean "the store is empty", and a
    // connection on it, so "no connection persisted" cannot mean "nothing works".
    let live_host = create_host(&router, &plain_host("live", "GitHubEnterpriseServer")).await;
    let live_id = assert_host_arn("live", &live_host);
    let (status, _) = call(
        &router,
        "CreateConnection",
        serde_json::json!({
            "ConnectionName": "on-live-host",
            "ProviderType": "GitHubEnterpriseServer",
            "HostArn": live_host,
        }),
    )
    .await;
    assert_eq!(status, 200, "the live host must resolve, got {status}");

    let unresolvable = [
        (
            "an id this store never minted",
            format!("arn:aws:codeconnections:{REGION}:{account}:host/0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0"),
        ),
        (
            "the codestar-connections namespace the pattern also admits",
            format!("arn:aws:codestar-connections:{REGION}:{account}:host/{live_id}"),
        ),
        (
            "the live host's id in another region",
            format!("arn:aws:codeconnections:us-east-1:{account}:host/{live_id}"),
        ),
        (
            "the live host's id in the aws-cn partition",
            format!("arn:aws-cn:codeconnections:cn-north-1:{account}:host/{live_id}"),
        ),
        (
            "the live host's id in another account",
            format!("arn:aws:codeconnections:{REGION}:210987654321:host/{live_id}"),
        ),
        (
            "a resource id that is not a host id at all",
            format!("arn:aws:codeconnections:{REGION}:{account}:host/x"),
        ),
    ];

    for (index, (label, host_arn)) in unresolvable.iter().enumerate() {
        let (status, body) = call(
            &router,
            "CreateConnection",
            serde_json::json!({
                "ConnectionName": format!("refused-{index}"),
                "ProviderType": "GitHubEnterpriseServer",
                "HostArn": host_arn,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 from CreateConnection, got {status} {body}"
        );
        assert!(
            body["__type"]
                .as_str()
                .is_some_and(|kind| kind.ends_with("ResourceNotFoundException")),
            "case {label:?}: expected ResourceNotFoundException, got {body}"
        );

        let (status, body) = call(
            &router,
            "GetHost",
            serde_json::json!({ "HostArn": host_arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 from GetHost, got {status} {body}"
        );
        assert!(
            body["__type"]
                .as_str()
                .is_some_and(|kind| kind.ends_with("ResourceNotFoundException")),
            "case {label:?}: expected ResourceNotFoundException, got {body}"
        );

        // The same rule where the ARN is a FILTER rather than the addressed resource:
        // `ListConnections` resolves `HostArnFilter` against the host store too, and
        // refuses one that names no host with the same `ResourceNotFoundException`
        // rather than reporting an empty page.
        let (status, body) = call(
            &router,
            "ListConnections",
            serde_json::json!({ "HostArnFilter": host_arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 from ListConnections, got {status} {body}"
        );
        assert!(
            body["__type"]
                .as_str()
                .is_some_and(|kind| kind.ends_with("ResourceNotFoundException")),
            "case {label:?}: expected ResourceNotFoundException, got {body}"
        );
        assert!(
            body["Connections"].is_null(),
            "case {label:?}: a refused filter reports no connections, got {body}"
        );
    }

    // Every refused create persisted nothing: only the one accepted connection is there.
    let (status, listed) = call(&router, "ListConnections", serde_json::json!({})).await;
    assert_eq!(
        status, 200,
        "ListConnections: expected 200, got {status} {listed}"
    );
    let names: Vec<&str> = listed["Connections"]
        .as_array()
        .unwrap_or_else(|| panic!("Connections is not a list: {listed}"))
        .iter()
        .map(|connection| connection["ConnectionName"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        names,
        vec!["on-live-host"],
        "a refused create must persist nothing, got {names:?}"
    );
}

/// Hosts and connections are separate resources: a connection ARN names no host, and
/// an ARN is resolved against the store its resource type belongs to.
#[tokio::test]
async fn a_connection_arn_does_not_resolve_as_a_host() {
    let router = router();
    let connection = create(&router, "just-a-connection", "GitHub").await;
    let host = create_host(&router, &plain_host("just-a-host", "GitLabSelfManaged")).await;

    // A connection ARN does not match the modeled `HostArn` pattern, so the request is
    // refused before this handler runs: the only requirement is that it is a 4xx and
    // never reports a host.
    let (status, body) = call(
        &router,
        "GetHost",
        serde_json::json!({ "HostArn": connection }),
    )
    .await;
    assert!(
        (400..500).contains(&status),
        "case connection-arn-to-GetHost: expected a client error, got {status} {body}"
    );
    assert!(
        body["Name"].is_null(),
        "case connection-arn-to-GetHost: no host may be reported, got {body}"
    );

    // And a host ARN names no connection: the modelled `ConnectionArn` is this service's
    // connection-scoped expression, so a host ARN is refused as invalid input before the
    // handler runs — a 400 that reports no connection. `refusal_messages.rs` pins the
    // clause it states.
    let (status, body) = call(
        &router,
        "GetConnection",
        serde_json::json!({ "ConnectionArn": host }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case host-arn-to-GetConnection: expected 400, got {status} {body}"
    );
    assert!(
        body["__type"]
            .as_str()
            .is_some_and(|kind| kind.ends_with("ValidationException")),
        "case host-arn-to-GetConnection: expected ValidationException, got {body}"
    );
    assert!(
        body["Connection"].is_null(),
        "case host-arn-to-GetConnection: no connection may be reported, got {body}"
    );
}
