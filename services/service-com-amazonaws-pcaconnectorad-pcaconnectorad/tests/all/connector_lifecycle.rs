// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the pca-connector-ad connector/template slice.
//!
//! Every case drives the real wire path: a `restJson1` request through the same
//! `MultiServiceRouter::dispatch` the production front door uses, so routing,
//! `@httpLabel` binding, constraint validation, the handler and serialization are
//! all exercised. Nothing reaches into the state struct. The request path itself
//! — the router, the request builder, the `@httpLabel` encoding and the error-code
//! read — comes from [`super::harness`], which every suite in this crate shares.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    call as dispatch, encode_label, register_directory, router, ACCOUNT, REGION,
};

// The store bounds the handlers refuse past, imported from the crate so these
// tests cannot drift from the caps the handlers enforce.
use service_com_amazonaws_pcaconnectorad_pcaconnectorad::state::{
    CONNECTOR_ACTIVATION_DELAY, MAX_CONNECTORS, MAX_TEMPLATES_PER_CONNECTOR,
};

/// The Microsoft AD certificate-template OID arc a template's `ObjectIdentifier`
/// sits under (see `state::template_object_identifier`).
const AD_TEMPLATE_OID_ARC: &str = "1.3.6.1.4.1.311.21.8";

/// Past the handler's activation delay by a wide margin, so a connector read
/// after it has certainly settled. Derived from the crate's own constant, so the
/// case cannot drift from the span the handler stores.
const PAST_ACTIVATION: std::time::Duration =
    CONNECTOR_ACTIVATION_DELAY.saturating_add(std::time::Duration::from_millis(750));

/// Status plus parsed JSON body of a dispatched request, with the wire error code
/// surfaced as `__code` so a case can assert it without caring whether the
/// protocol put it in the header or the body.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let (status, code, parsed) = dispatch(router, method, path_and_query, body).await;
    let parsed = match (code.is_empty(), parsed) {
        (true, parsed) => parsed,
        (false, Value::Object(mut map)) => {
            map.insert("__code".to_string(), Value::String(code));
            Value::Object(map)
        }
        (false, _) => json!({ "__code": code }),
    };
    (status, parsed)
}

/// Whether `value` is a lowercase hyphenated random (version 4) UUID, the form
/// the connector and template ARN patterns require: 8-4-4-4-12 hex groups, the
/// version nibble `4` leading the third group and an RFC 4122 §4.4 variant
/// nibble (`8`, `9`, `a` or `b`) leading the fourth.
fn is_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    let shaped = groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| {
                group.len() == *len
                    && group
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            });
    shaped
        && groups[2].starts_with('4')
        && groups[3]
            .chars()
            .next()
            .is_some_and(|variant| matches!(variant, '8' | '9' | 'a' | 'b'))
}

fn connector_arn_prefix() -> String {
    format!("arn:aws:pca-connector-ad:{REGION}:{ACCOUNT}:connector/")
}

/// One row of the connector table: inputs that differ along what the rule
/// generalises over (directory id, CA, security-group count, IP address type,
/// tag count).
struct ConnectorCase {
    name: &'static str,
    directory_id: &'static str,
    certificate_authority_arn: &'static str,
    security_group_ids: &'static [&'static str],
    ip_address_type: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
}

/// `IpAddressType` values come from the modelled enum (`IP_ADDRESS_TYPE_VALUES`:
/// `IPV4`, `DUALSTACK`); `SecurityGroupIds` is modelled `length 1..4`, so the
/// rows span both published bounds.
const CONNECTOR_CASES: &[ConnectorCase] = &[
    ConnectorCase {
        name: "one security group, no ip address type, no tags",
        directory_id: "d-0123456789",
        certificate_authority_arn:
            "arn:aws:acm-pca:us-west-2:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555",
        security_group_ids: &["sg-0123456789abcdef0"],
        ip_address_type: None,
        tags: &[],
    },
    ConnectorCase {
        name: "four security groups at the modelled maximum, IPV4, one tag",
        directory_id: "d-abcdef0123",
        certificate_authority_arn:
            "arn:aws:acm-pca:us-west-2:000000000000:certificate-authority/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
        security_group_ids: &[
            "sg-00000000000000001",
            "sg-00000000000000002",
            "sg-00000000000000003",
            "sg-00000000000000004",
        ],
        ip_address_type: Some("IPV4"),
        tags: &[("Team", "pki")],
    },
    ConnectorCase {
        name: "two security groups, DUALSTACK, several tags",
        directory_id: "d-00000000ff",
        certificate_authority_arn:
            "arn:aws:acm-pca:us-west-2:000000000000:certificate-authority/0f0f0f0f-1e1e-2d2d-3c3c-4b4b4b4b4b4b",
        security_group_ids: &["sg-000000000000000ab", "sg-000000000000000cd"],
        ip_address_type: Some("DUALSTACK"),
        tags: &[("Env", "test"), ("Owner", "ad")],
    },
];

impl ConnectorCase {
    fn body(&self) -> Value {
        let mut vpc = json!({ "SecurityGroupIds": self.security_group_ids });
        if let Some(ip_address_type) = self.ip_address_type {
            vpc["IpAddressType"] = json!(ip_address_type);
        }
        let mut body = json!({
            "DirectoryId": self.directory_id,
            "CertificateAuthorityArn": self.certificate_authority_arn,
            "VpcInformation": vpc,
        });
        if !self.tags.is_empty() {
            body["Tags"] = Value::Object(
                self.tags
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), json!(v)))
                    .collect(),
            );
        }
        body
    }
}

/// `CreateConnector` for `body`, answering the ARN it minted.
///
/// `pub(crate)` because the sibling suites that need a connector to hang a child
/// resource off need the same prerequisite chain, and a second copy of it is how two
/// suites come to set up two different things.
pub(crate) async fn create_connector(router: &MultiServiceRouter, body: &Value) -> String {
    // A connector's directory must already be registered with this service: the
    // create resolves its `DirectoryId` against the directory-registration store,
    // so the prerequisite is established before every case below.
    register_directory(
        router,
        body["DirectoryId"]
            .as_str()
            .unwrap_or_else(|| panic!("a connector body must carry a DirectoryId: {body}")),
    )
    .await;
    let (status, parsed) = call(router, "POST", "/connectors", Some(body)).await;
    assert_eq!(
        status, 202,
        "CreateConnector must answer the modelled 202, got {status} / {parsed}"
    );
    parsed["ConnectorArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateConnector returned no ConnectorArn: {parsed}"))
        .to_string()
}

/// The `Arn` of every summary in a `ListConnectors` response, in the order the
/// service returned them.
fn listed_arns(listed: &Value) -> Vec<String> {
    listed["Connectors"]
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .filter_map(|summary| summary["Arn"].as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

async fn get_connector(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/connectors/{}", encode_label(arn)),
        None,
    )
    .await
}

#[tokio::test]
async fn create_connector_mints_a_modelled_arn_and_get_connector_reads_the_record_back() {
    let router = router();

    for case in CONNECTOR_CASES {
        let body = case.body();
        let started = std::time::Instant::now();
        let arn = create_connector(&router, &body).await;

        let prefix = connector_arn_prefix();
        assert!(
            arn.starts_with(&prefix),
            "case {}: expected a connector ARN starting {prefix}, got {arn:?}",
            case.name
        );
        assert!(
            is_uuid(&arn[prefix.len()..]),
            "case {}: expected a uuid after `connector/`, got {arn:?}",
            case.name
        );

        let (status, parsed) = get_connector(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {}: expected GetConnector 200, got {status} / {parsed}",
            case.name
        );
        let connector = &parsed["Connector"];
        assert_eq!(
            connector["Arn"].as_str(),
            Some(arn.as_str()),
            "case {}: GetConnector must report the ARN it was asked for, got {connector}",
            case.name
        );
        assert_eq!(
            connector["DirectoryId"], body["DirectoryId"],
            "case {}: DirectoryId must be what CreateConnector persisted, got {connector}",
            case.name
        );
        assert_eq!(
            connector["CertificateAuthorityArn"], body["CertificateAuthorityArn"],
            "case {}: CertificateAuthorityArn must round-trip, got {connector}",
            case.name
        );
        assert_eq!(
            connector["VpcInformation"], body["VpcInformation"],
            "case {}: VpcInformation must round-trip, got {connector}",
            case.name
        );
        // The connector is CREATING until the clock-owed activation delay has
        // passed; the read below is well inside it unless the host is so loaded
        // that the whole round trip outran the delay, in which case the settled
        // value is the correct one. The dedicated settle case proves the move.
        let elapsed = started.elapsed();
        let expected_status = if elapsed < CONNECTOR_ACTIVATION_DELAY {
            "CREATING"
        } else {
            "ACTIVE"
        };
        assert_eq!(
            connector["Status"].as_str(),
            Some(expected_status),
            "case {}: a connector read {elapsed:?} after CreateConnector (delay \
             {CONNECTOR_ACTIVATION_DELAY:?}) must be {expected_status}, got {connector}",
            case.name
        );
        assert!(
            connector["CreatedAt"].is_number(),
            "case {}: CreatedAt must be set, got {connector}",
            case.name
        );
        assert_eq!(
            connector["CreatedAt"], connector["UpdatedAt"],
            "case {}: an untouched connector has UpdatedAt == CreatedAt, got {connector}",
            case.name
        );
        let endpoint = connector["CertificateEnrollmentPolicyServerEndpoint"]
            .as_str()
            .unwrap_or_default();
        let connector_id = &arn[prefix.len()..];
        assert!(
            endpoint.contains(connector_id) && endpoint.contains(REGION),
            "case {}: the enrollment endpoint must name the connector and its region, got {endpoint:?}",
            case.name
        );
        assert!(
            connector["StatusReason"].is_null(),
            "case {}: a connector that did not fail carries no StatusReason, got {connector}",
            case.name
        );
    }
}

#[tokio::test]
async fn get_connector_on_an_arn_that_was_never_created_is_resource_not_found() {
    let router = router();
    // Well-formed connector ARNs the service never minted: a different uuid, a
    // different region, and one created-then-deleted below in its own case.
    let unknown = [
        format!("{}11111111-2222-3333-4444-555555555555", connector_arn_prefix()),
        format!("arn:aws:pca-connector-ad:eu-west-1:{ACCOUNT}:connector/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"),
        format!("{}00000000-0000-0000-0000-000000000000", connector_arn_prefix()),
    ];

    for arn in &unknown {
        let (status, parsed) = get_connector(&router, arn).await;
        assert_eq!(
            status, 404,
            "case {arn}: an unknown connector ARN is a 404, got {status} / {parsed}"
        );
        assert_eq!(
            parsed["__code"].as_str(),
            Some("ResourceNotFoundException"),
            "case {arn}: expected ResourceNotFoundException, got {parsed}"
        );
        assert_eq!(
            parsed["ResourceType"].as_str(),
            Some("Connector"),
            "case {arn}: the refusal must name the resource type, got {parsed}"
        );
    }
}

#[tokio::test]
async fn a_connector_settles_from_creating_to_active_after_its_activation_delay() {
    let router = router();
    // The CREATING observation is owed only while less than the activation delay
    // has elapsed since the create; on a loaded machine the two dispatches can
    // themselves take longer than that, so the case asserts against the time the
    // round trip actually took rather than flaking on a slow build host.
    let started = std::time::Instant::now();
    let arn = create_connector(&router, &CONNECTOR_CASES[0].body()).await;

    let (_, immediately) = get_connector(&router, &arn).await;
    let elapsed = started.elapsed();
    let expected_status = if elapsed < CONNECTOR_ACTIVATION_DELAY {
        "CREATING"
    } else {
        "ACTIVE"
    };
    assert_eq!(
        immediately["Connector"]["Status"].as_str(),
        Some(expected_status),
        "a connector read {elapsed:?} after CreateConnector (delay {CONNECTOR_ACTIVATION_DELAY:?}) \
         must be {expected_status}, got {immediately}"
    );

    tokio::time::sleep(PAST_ACTIVATION).await;

    let (_, settled) = get_connector(&router, &arn).await;
    assert_eq!(
        settled["Connector"]["Status"].as_str(),
        Some("ACTIVE"),
        "after the activation delay the connector must be ACTIVE, got {settled}"
    );
    assert_eq!(
        settled["Connector"]["Arn"], immediately["Connector"]["Arn"],
        "settling must not change the connector's identity, got {settled}"
    );
}

#[tokio::test]
async fn list_connectors_pages_through_every_connector_once_in_creation_order() {
    let router = router();

    // Six connectors, so page sizes below span "divides evenly", "leaves a
    // remainder" and "larger than the collection".
    let mut created = Vec::new();
    for index in 0..6 {
        let case = &CONNECTOR_CASES[index % CONNECTOR_CASES.len()];
        created.push(create_connector(&router, &case.body()).await);
    }

    for page_size in [1usize, 2, 4, 6, 1000] {
        let mut seen = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => format!(
                    "/connectors?MaxResults={page_size}&NextToken={}",
                    encode_label(token)
                ),
                None => format!("/connectors?MaxResults={page_size}"),
            };
            let (status, parsed) = call(&router, "GET", &query, None).await;
            assert_eq!(
                status, 200,
                "case page_size={page_size}: ListConnectors must answer 200, got {status} / {parsed}"
            );
            let page = parsed["Connectors"]
                .as_array()
                .unwrap_or_else(|| panic!("case page_size={page_size}: no Connectors: {parsed}"));
            assert!(
                !page.is_empty() && page.len() <= page_size,
                "case page_size={page_size}: a page holds 1..={page_size} items, got {}",
                page.len()
            );
            for summary in page {
                seen.push(
                    summary["Arn"]
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case page_size={page_size}: summary without Arn: {summary}")
                        })
                        .to_string(),
                );
            }
            pages += 1;
            assert!(
                pages <= created.len() + 1,
                "case page_size={page_size}: paging did not terminate after {pages} pages"
            );
            match parsed["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, created,
            "case page_size={page_size}: paging must yield every connector exactly once in creation order"
        );
        assert_eq!(
            seen.iter().collect::<BTreeSet<_>>().len(),
            created.len(),
            "case page_size={page_size}: paging must not duplicate a connector, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn list_connectors_refuses_a_next_token_it_never_issued() {
    let router = router();
    create_connector(&router, &CONNECTOR_CASES[0].body()).await;

    // Every value here satisfies the modelled `NextToken` pattern (base64url
    // characters in groups of four), so the refusal comes from the handler's
    // cursor check and not from the boundary validator. The rows span the ways a
    // token can be foreign: a value with no service tag at all, the tag alone,
    // the tag with a cursor field of the wrong width, the tag with a field that
    // is not hex, a wrong-case tag, and a bare cursor field — the shape a
    // service without a tagged token would silently honour.
    let foreign = [
        "YWJj",
        "AAAA",
        "X-_a",
        "Nw==",
        "pcad",
        "pcad00000000000000010000",
        "pcadzzzzzzzzzzzzzzzz",
        "pcaD0000000000000001",
        "0000000000000001",
    ];
    for token in foreign {
        let (status, parsed) = call(
            &router,
            "GET",
            &format!("/connectors?NextToken={}", encode_label(token)),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {token}: a token this service never issued is a 400, got {status} / {parsed}"
        );
        assert_eq!(
            parsed["__code"].as_str(),
            Some("ValidationException"),
            "case {token}: expected ValidationException, got {parsed}"
        );
        // One wording for this error class across this service's list operations
        // (`state::pagination_token_message`): the fixed sentence, the operation
        // from the request, then the schema the presented token declares. The
        // layer that reads the token keys it at the lowercase `message` member.
        assert!(
            parsed["message"]
                .as_str()
                .unwrap_or_default()
                .starts_with("Pagination token exception in operation 'ListConnectors': "),
            "case {token}: the refusal must read as a pagination-token failure of this \
             operation, got {parsed}"
        );
    }
}

/// One row of the template table: the definition union version, which fixes the
/// `PolicySchema` the service must report.
struct TemplateCase {
    name: &'static str,
    expected_policy_schema: i64,
    definition: fn() -> Value,
}

fn certificate_validity() -> Value {
    // `ValidityPeriodType` values come from the modelled enum
    // (`VALIDITY_PERIOD_TYPE_VALUES`).
    json!({
        "ValidityPeriod": { "PeriodType": "YEARS", "Period": 1 },
        "RenewalPeriod": { "PeriodType": "WEEKS", "Period": 6 },
    })
}

fn template_v2() -> Value {
    json!({ "TemplateV2": {
        "CertificateValidity": certificate_validity(),
        "PrivateKeyAttributes": { "MinimalKeyLength": 2048, "KeySpec": "KEY_EXCHANGE" },
        "PrivateKeyFlags": { "ClientVersion": "WINDOWS_SERVER_2016" },
        "EnrollmentFlags": { "NoSecurityExtension": true },
        "SubjectNameFlags": { "SanRequireDirectoryGuid": true },
        "GeneralFlags": { "MachineType": true },
        "Extensions": { "KeyUsage": { "UsageFlags": { "KeyEncipherment": true, "DigitalSignature": true } } },
    }})
}

fn template_v3() -> Value {
    json!({ "TemplateV3": {
        "CertificateValidity": certificate_validity(),
        "PrivateKeyAttributes": {
            "MinimalKeyLength": 2048,
            "KeySpec": "KEY_EXCHANGE",
            "KeyUsageProperty": { "PropertyType": "ALL" },
            "Algorithm": "RSA",
        },
        "PrivateKeyFlags": { "ClientVersion": "WINDOWS_SERVER_2016" },
        "EnrollmentFlags": { "NoSecurityExtension": true },
        "SubjectNameFlags": { "SanRequireDirectoryGuid": true },
        "GeneralFlags": { "MachineType": true },
        "HashAlgorithm": "SHA256",
        "Extensions": { "KeyUsage": { "UsageFlags": { "KeyEncipherment": true } } },
    }})
}

fn template_v4() -> Value {
    json!({ "TemplateV4": {
        "CertificateValidity": certificate_validity(),
        "PrivateKeyAttributes": { "MinimalKeyLength": 2048, "KeySpec": "KEY_EXCHANGE" },
        "PrivateKeyFlags": { "ClientVersion": "WINDOWS_SERVER_2016" },
        "EnrollmentFlags": { "NoSecurityExtension": true },
        "SubjectNameFlags": { "SanRequireDirectoryGuid": true },
        "GeneralFlags": { "MachineType": true },
        "Extensions": { "KeyUsage": { "UsageFlags": { "KeyEncipherment": true } } },
    }})
}

/// The `TemplateDefinition` union's members are `TemplateV2`, `TemplateV3` and
/// `TemplateV4`; the schema version AWS reports is the version of the member
/// supplied, so the table covers every one of them.
const TEMPLATE_CASES: &[TemplateCase] = &[
    TemplateCase {
        name: "TemplateV2",
        expected_policy_schema: 2,
        definition: template_v2,
    },
    TemplateCase {
        name: "TemplateV3",
        expected_policy_schema: 3,
        definition: template_v3,
    },
    TemplateCase {
        name: "TemplateV4",
        expected_policy_schema: 4,
        definition: template_v4,
    },
];

async fn create_template(
    router: &MultiServiceRouter,
    connector_arn: &str,
    name: &str,
    definition: &Value,
) -> (u16, Value) {
    let body = json!({
        "ConnectorArn": connector_arn,
        "Name": name,
        "Definition": definition,
    });
    call(router, "POST", "/templates", Some(&body)).await
}

#[tokio::test]
async fn create_template_persists_the_definition_and_get_template_reads_it_back() {
    let router = router();
    let connector_arn = create_connector(&router, &CONNECTOR_CASES[0].body()).await;

    for case in TEMPLATE_CASES {
        let definition = (case.definition)();
        let template_name = format!("Template-{}", case.name);
        let (status, parsed) =
            create_template(&router, &connector_arn, &template_name, &definition).await;
        assert_eq!(
            status, 200,
            "case {}: CreateTemplate must answer the modelled 200, got {status} / {parsed}",
            case.name
        );
        let template_arn = parsed["TemplateArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {}: no TemplateArn: {parsed}", case.name))
            .to_string();
        let child_prefix = format!("{connector_arn}/template/");
        assert!(
            template_arn.starts_with(&child_prefix)
                && is_uuid(&template_arn[child_prefix.len()..]),
            "case {}: expected `<connectorArn>/template/<uuid>`, got {template_arn:?}",
            case.name
        );

        let (status, parsed) = call(
            &router,
            "GET",
            &format!("/templates/{}", encode_label(&template_arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {}: expected GetTemplate 200, got {status} / {parsed}",
            case.name
        );
        let template = &parsed["Template"];
        assert_eq!(
            template["Arn"].as_str(),
            Some(template_arn.as_str()),
            "case {}: GetTemplate must report the ARN it was asked for, got {template}",
            case.name
        );
        assert_eq!(
            template["ConnectorArn"].as_str(),
            Some(connector_arn.as_str()),
            "case {}: the template must name its parent connector, got {template}",
            case.name
        );
        assert_eq!(
            template["Name"].as_str(),
            Some(template_name.as_str()),
            "case {}: Name must round-trip, got {template}",
            case.name
        );
        assert_eq!(
            template["Definition"], definition,
            "case {}: Definition must come back exactly as supplied, got {template}",
            case.name
        );
        assert_eq!(
            template["PolicySchema"].as_i64(),
            Some(case.expected_policy_schema),
            "case {}: PolicySchema is the definition's own version, got {template}",
            case.name
        );
        assert_eq!(
            template["Status"].as_str(),
            Some("ACTIVE"),
            "case {}: TemplateStatus models only ACTIVE/DELETING, so a new template is ACTIVE, got {template}",
            case.name
        );
        assert_eq!(
            template["Revision"],
            json!({ "MajorRevision": 1, "MinorRevision": 0 }),
            "case {}: a never-updated template sits at its first revision, got {template}",
            case.name
        );
        let oid = template["ObjectIdentifier"].as_str().unwrap_or_default();
        assert!(
            oid.starts_with(&format!("{AD_TEMPLATE_OID_ARC}.")),
            "case {}: the template OID must sit under the AD template arc, got {oid:?}",
            case.name
        );
        assert!(
            oid.split('.').all(|part| !part.is_empty()
                && part.bytes().all(|b| b.is_ascii_digit())),
            "case {}: an OID is dot-separated non-negative integers, got {oid:?}",
            case.name
        );
        assert!(
            template["CreatedAt"].is_number() && template["UpdatedAt"].is_number(),
            "case {}: CreatedAt/UpdatedAt must be set, got {template}",
            case.name
        );
    }
}

#[tokio::test]
async fn template_names_are_unique_per_connector_and_free_on_another_connector() {
    let router = router();
    let first = create_connector(&router, &CONNECTOR_CASES[0].body()).await;
    let second = create_connector(&router, &CONNECTOR_CASES[1].body()).await;

    for case in TEMPLATE_CASES {
        let definition = (case.definition)();
        let name = format!("Shared-{}", case.name);

        let (status, parsed) = create_template(&router, &first, &name, &definition).await;
        assert_eq!(
            status, 200,
            "case {}: the first use of a name must succeed, got {status} / {parsed}",
            case.name
        );

        let (status, parsed) = create_template(&router, &first, &name, &definition).await;
        assert_eq!(
            status, 409,
            "case {}: a duplicate name on the same connector is a 409, got {status} / {parsed}",
            case.name
        );
        assert_eq!(
            parsed["__code"].as_str(),
            Some("ConflictException"),
            "case {}: expected ConflictException, got {parsed}",
            case.name
        );

        let (status, parsed) = create_template(&router, &second, &name, &definition).await;
        assert_eq!(
            status, 200,
            "case {}: the same name on another connector is free, got {status} / {parsed}",
            case.name
        );
    }
}

#[tokio::test]
async fn create_template_on_a_connector_that_does_not_exist_is_resource_not_found() {
    let router = router();
    let unknown = format!(
        "{}99999999-8888-7777-6666-555555555555",
        connector_arn_prefix()
    );

    for case in TEMPLATE_CASES {
        let (status, parsed) =
            create_template(&router, &unknown, "Orphan", &(case.definition)()).await;
        assert_eq!(
            status, 404,
            "case {}: an unknown ConnectorArn is a 404, got {status} / {parsed}",
            case.name
        );
        assert_eq!(
            parsed["__code"].as_str(),
            Some("ResourceNotFoundException"),
            "case {}: expected ResourceNotFoundException, got {parsed}",
            case.name
        );
    }
}

#[tokio::test]
async fn delete_connector_removes_it_with_its_templates_and_stays_idempotent() {
    let router = router();
    let doomed = create_connector(&router, &CONNECTOR_CASES[0].body()).await;
    let survivor = create_connector(&router, &CONNECTOR_CASES[2].body()).await;

    let mut doomed_templates = Vec::new();
    for case in TEMPLATE_CASES {
        let (_, parsed) = create_template(
            &router,
            &doomed,
            &format!("Doomed-{}", case.name),
            &(case.definition)(),
        )
        .await;
        doomed_templates.push(parsed["TemplateArn"].as_str().unwrap_or_default().to_string());
    }
    let (_, survivor_template) = create_template(
        &router,
        &survivor,
        "Survivor",
        &(TEMPLATE_CASES[0].definition)(),
    )
    .await;
    let survivor_template_arn = survivor_template["TemplateArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Two deletes of the same ARN, and one of an ARN that never existed: all 202.
    let never_existed = format!(
        "{}12121212-3434-5656-7878-909090909090",
        connector_arn_prefix()
    );
    for (attempt, arn) in [&doomed, &doomed, &never_existed].iter().enumerate() {
        let (status, parsed) = call(
            &router,
            "DELETE",
            &format!("/connectors/{}", encode_label(arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 202,
            "case attempt {attempt} on {arn}: @idempotent DeleteConnector always answers 202, got {status} / {parsed}"
        );
    }

    let (status, parsed) = get_connector(&router, &doomed).await;
    assert_eq!(
        status, 404,
        "a deleted connector must read back as 404, got {status} / {parsed}"
    );

    for template_arn in &doomed_templates {
        let (status, parsed) = call(
            &router,
            "GET",
            &format!("/templates/{}", encode_label(template_arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {template_arn}: a deleted connector's templates go with it, got {status} / {parsed}"
        );
    }

    let (status, parsed) = call(
        &router,
        "GET",
        &format!("/templates/{}", encode_label(&survivor_template_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "another connector's template must survive, got {status} / {parsed}"
    );

    let (status, listed) = call(&router, "GET", "/connectors", None).await;
    assert_eq!(status, 200, "ListConnectors must answer 200, got {listed}");
    let arns = listed_arns(&listed);
    assert_eq!(
        arns,
        vec![survivor.clone()],
        "after the delete only the surviving connector is listed, got {listed}"
    );
}

#[tokio::test]
async fn a_repeated_client_token_returns_the_first_resource_rather_than_a_second_one() {
    let router = router();

    // Connector: the same token twice must name one connector, and a different
    // token must mint another.
    let mut body = CONNECTOR_CASES[0].body();
    body["ClientToken"] = json!("token-connector-1");
    let first = create_connector(&router, &body).await;
    let repeat = create_connector(&router, &body).await;
    assert_eq!(
        first, repeat,
        "case ClientToken reuse: a retried CreateConnector must return the first ARN, got {first} then {repeat}"
    );

    let mut other = CONNECTOR_CASES[1].body();
    other["ClientToken"] = json!("token-connector-2");
    let second = create_connector(&router, &other).await;
    assert_ne!(
        first, second,
        "case distinct ClientTokens: a new token must mint a new connector, got {second}"
    );

    let (_, listed) = call(&router, "GET", "/connectors", None).await;
    let arns = listed_arns(&listed);
    assert_eq!(
        arns,
        vec![first.clone(), second.clone()],
        "two tokens over three calls must leave exactly two connectors, got {listed}"
    );

    // Template: the same rule, on the child resource.
    for case in TEMPLATE_CASES {
        let definition = (case.definition)();
        let body = json!({
            "ConnectorArn": first,
            "Name": format!("Idempotent-{}", case.name),
            "Definition": definition,
            "ClientToken": format!("token-template-{}", case.name),
        });
        let (status, created) = call(&router, "POST", "/templates", Some(&body)).await;
        assert_eq!(
            status, 200,
            "case {}: CreateTemplate must answer 200, got {status} / {created}",
            case.name
        );
        let (status, retried) = call(&router, "POST", "/templates", Some(&body)).await;
        assert_eq!(
            status, 200,
            "case {}: a retried CreateTemplate must answer 200, got {status} / {retried}",
            case.name
        );
        assert_eq!(
            created["TemplateArn"], retried["TemplateArn"],
            "case {}: a retried CreateTemplate must return the first ARN, got {created} then {retried}",
            case.name
        );

        // The same token against a DIFFERENT connector addresses a different
        // resource, so it must mint a new template under that connector rather
        // than answer with the first connector's ARN.
        let mut elsewhere = body.clone();
        elsewhere["ConnectorArn"] = json!(second);
        let (status, other) = call(&router, "POST", "/templates", Some(&elsewhere)).await;
        assert_eq!(
            status, 200,
            "case {}: the same token on another connector must succeed, got {status} / {other}",
            case.name
        );
        assert_ne!(
            created["TemplateArn"], other["TemplateArn"],
            "case {}: a token reused against another connector must not return the first connector's template, got {other}",
            case.name
        );
        let other_arn = other["TemplateArn"].as_str().unwrap_or_default();
        assert!(
            other_arn.starts_with(&format!("{second}/template/")),
            "case {}: the new template must be a child of the connector addressed, got {other_arn:?}",
            case.name
        );
    }
}

#[tokio::test]
async fn the_connector_store_is_bounded_and_refuses_past_its_quota() {
    let router = router();
    // The bound is on connectors, and several connectors may share one directory,
    // so the prerequisite registrations are established once up front.
    for case in CONNECTOR_CASES {
        register_directory(&router, case.directory_id).await;
    }

    for index in 0..MAX_CONNECTORS {
        let case = &CONNECTOR_CASES[index % CONNECTOR_CASES.len()];
        let (status, parsed) = call(&router, "POST", "/connectors", Some(&case.body())).await;
        assert_eq!(
            status, 202,
            "case connector {index}: every create up to the bound must succeed, got {status} / {parsed}"
        );
    }

    let (status, parsed) = call(
        &router,
        "POST",
        "/connectors",
        Some(&CONNECTOR_CASES[0].body()),
    )
    .await;
    assert_eq!(
        status, 402,
        "case one past the bound: the create must be refused with the modelled quota status, got {status} / {parsed}"
    );
    assert_eq!(
        parsed["__code"].as_str(),
        Some("ServiceQuotaExceededException"),
        "case one past the bound: expected ServiceQuotaExceededException, got {parsed}"
    );
    assert_eq!(
        parsed["ResourceType"].as_str(),
        Some("Connector"),
        "case one past the bound: the refusal must name the resource type, got {parsed}"
    );

    // The refusal did not grow the store: exactly the bound is held.
    let (_, listed) = call(
        &router,
        "GET",
        &format!("/connectors?MaxResults={MAX_CONNECTORS}"),
        None,
    )
    .await;
    assert_eq!(
        listed_arns(&listed).len(),
        MAX_CONNECTORS,
        "a refused create must not have been stored, got {} connectors",
        listed_arns(&listed).len()
    );
}

#[tokio::test]
async fn the_template_store_is_bounded_per_connector_and_refuses_past_its_quota() {
    let router = router();
    let connector_arn = create_connector(&router, &CONNECTOR_CASES[0].body()).await;
    let other_arn = create_connector(&router, &CONNECTOR_CASES[1].body()).await;
    let definition = (TEMPLATE_CASES[0].definition)();

    for index in 0..MAX_TEMPLATES_PER_CONNECTOR {
        let (status, parsed) = create_template(
            &router,
            &connector_arn,
            &format!("Bounded-{index}"),
            &definition,
        )
        .await;
        assert_eq!(
            status, 200,
            "case template {index}: every create up to the bound must succeed, got {status} / {parsed}"
        );
    }

    let (status, parsed) =
        create_template(&router, &connector_arn, "OnePastTheBound", &definition).await;
    assert_eq!(
        status, 402,
        "case one past the bound: the create must be refused with the modelled quota status, got {status} / {parsed}"
    );
    assert_eq!(
        parsed["__code"].as_str(),
        Some("ServiceQuotaExceededException"),
        "case one past the bound: expected ServiceQuotaExceededException, got {parsed}"
    );

    // The bound is per connector, so another connector is unaffected.
    let (status, parsed) =
        create_template(&router, &other_arn, "FirstOnAnotherConnector", &definition).await;
    assert_eq!(
        status, 200,
        "case another connector: the per-connector bound must not spill over, got {status} / {parsed}"
    );
}
