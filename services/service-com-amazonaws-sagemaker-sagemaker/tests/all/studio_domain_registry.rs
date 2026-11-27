// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's Studio DOMAIN registry: `CreateDomain`,
//! `DescribeDomain` and `ListDomains`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets. The harness (`Emulator`,
//! `error_code`, `message`, `str_field`) is the one `inference_deployment_lifecycle`
//! already defines for this target.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the
//! create/read round trip for every optional member and both `AuthMode` members across
//! two Regions and three name shapes; every member of each enum catalog the create takes,
//! crossed against a value outside it; the documented default of each member that has
//! one; the clock-owed `Pending` -> `InService` move; and the page walk for every page
//! size from one entry to past the published `MaxResults` maximum.
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::ops::types::{
    APP_NETWORK_ACCESS_TYPE_VALUES, APP_SECURITY_GROUP_MANAGEMENT_VALUES, AUTH_MODE_VALUES,
    HOME_EFS_FILE_SYSTEM_CREATION_VALUES, TAG_PROPAGATION_VALUES,
};
use service_com_amazonaws_sagemaker_sagemaker::state::{
    ComAmazonawsSagemakerSagemakerState, DEFAULT_APP_NETWORK_ACCESS_TYPE,
    DEFAULT_HOME_EFS_FILE_SYSTEM_CREATION, DEFAULT_TAG_PROPAGATION, DOMAIN_DEFAULT_PAGE_SIZE,
    DOMAIN_STATUS_IN_SERVICE, DOMAIN_STATUS_PENDING, MAX_PAGE_SIZE,
};

use crate::inference_deployment_lifecycle::{
    error_code, message, str_field, Emulator, ACCOUNT,
};

/// An execution role inside `RoleArn`'s modelled pattern, for the `DefaultUserSettings`
/// every create must carry.
const EXECUTION_ROLE: &str = "arn:aws:iam::123456789012:role/service-role/StudioRole";

/// How long a poll below gives the clock to settle a domain: far longer than the span the
/// state gives a domain's provisioning, so a loaded host still settles well inside it.
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// How often a poll looks again. Only the loop's condition decides when it stops.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The smallest `CreateDomain` request: the required members, plus the VPC and the subnet
/// a SageMaker AI domain's networking must name (`VpcId` and `SubnetIds` are optional only
/// for a domain created from SageMaker Unified Studio).
fn minimal(domain_name: &str) -> Value {
    json!({
        "DomainName": domain_name,
        "AuthMode": "IAM",
        "DefaultUserSettings": { "ExecutionRole": EXECUTION_ROLE },
        "VpcId": "vpc-0a1b2c3d",
        "SubnetIds": ["subnet-0a1b2c3d"]
    })
}

/// Call `target` in `region` and require a 200, returning the parsed output.
async fn ok_in(emulator: &Emulator, region: &str, target: &str, body: &Value) -> Value {
    let (status, parsed) = emulator.call_in(region, target, body).await;
    assert_eq!(
        status, 200,
        "case {target} in {region} {body}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// The `DomainId`s a `ListDomains` response lists, in response order.
fn listed_ids(body: &Value) -> Vec<String> {
    body.get("Domains")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a Domains list in {body}"))
        .iter()
        .map(|details| str_field(details, "DomainId"))
        .collect()
}

/// Whether `domain_id` has the form `DomainId`'s model pattern admits,
/// `^d-(-*[a-z0-9]){1,61}$`: a `d-` prefix, then 1..=61 lowercase alphanumerics with
/// hyphens allowed between them.
fn well_formed_domain_id(domain_id: &str) -> bool {
    let Some(tail) = domain_id.strip_prefix("d-") else {
        return false;
    };
    let alphanumerics = tail.chars().filter(|c| c.is_ascii_alphanumeric()).count();
    (1..=61).contains(&alphanumerics)
        && !tail.ends_with('-')
        && tail
            .chars()
            .all(|c| c == '-' || c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// One row of the round-trip table: a `CreateDomain` request, the Region it is signed
/// for, and the members `DescribeDomain` must replay for it.
struct RoundTrip {
    case: &'static str,
    region: &'static str,
    request: Value,
    /// `(member, expected JSON)` pairs the reader must report.
    expected: Vec<(&'static str, Value)>,
}

/// RULE: `CreateDomain` mints a server-generated `DomainId`, the `domain/<DomainId>` ARN
/// and the domain's Studio URL, stores every member the request carried and resolves the
/// three members the model documents a default for; `DescribeDomain` replays exactly
/// that, addressed by the minted id alone.
#[tokio::test]
async fn create_domain_round_trips_through_describe_domain() {
    let emulator = Emulator::new();

    // Rows differ along what the rule generalises over: both `AuthMode` members, the
    // Region signed for, the name shape (one character, hyphenated, the 63-character
    // maximum `DomainName`'s length constraint admits), how many subnets and tags the
    // request carries, and whether the optional members are present at all.
    let longest_name = format!("a{}", "b".repeat(62));
    let rows: Vec<RoundTrip> = vec![
        RoundTrip {
            case: "required-members-only",
            region: "us-east-1",
            request: minimal("studio"),
            expected: vec![
                ("DomainName", json!("studio")),
                ("AuthMode", json!("IAM")),
                // The documented defaults, resolved by the create rather than left out.
                (
                    "AppNetworkAccessType",
                    json!(DEFAULT_APP_NETWORK_ACCESS_TYPE),
                ),
                (
                    "HomeEfsFileSystemCreation",
                    json!(DEFAULT_HOME_EFS_FILE_SYSTEM_CREATION),
                ),
                ("TagPropagation", json!(DEFAULT_TAG_PROPAGATION)),
            ],
        },
        RoundTrip {
            case: "single-character-name-in-another-region",
            region: "eu-west-2",
            request: minimal("s"),
            expected: vec![("DomainName", json!("s")), ("AuthMode", json!("IAM"))],
        },
        RoundTrip {
            case: "longest-name-the-length-constraint-admits",
            region: "us-east-1",
            request: minimal(&longest_name),
            expected: vec![("DomainName", json!(longest_name.clone()))],
        },
        RoundTrip {
            case: "sso-auth-mode-with-one-subnet",
            region: "ap-southeast-2",
            request: json!({
                "DomainName": "sso-studio",
                "AuthMode": "SSO",
                "DefaultUserSettings": { "ExecutionRole": EXECUTION_ROLE },
                "VpcId": "vpc-0a1b2c3d",
                "SubnetIds": ["subnet-0a1b2c3d"]
            }),
            expected: vec![
                ("AuthMode", json!("SSO")),
                ("VpcId", json!("vpc-0a1b2c3d")),
                ("SubnetIds", json!(["subnet-0a1b2c3d"])),
            ],
        },
        RoundTrip {
            case: "every-optional-member-present",
            region: "us-west-2",
            request: json!({
                "DomainName": "vpc-only-studio",
                "AuthMode": "IAM",
                "DefaultUserSettings": {
                    "ExecutionRole": EXECUTION_ROLE,
                    "SecurityGroups": ["sg-01", "sg-02"]
                },
                "DefaultSpaceSettings": { "ExecutionRole": EXECUTION_ROLE },
                "DomainSettings": { "SecurityGroupIds": ["sg-03"] },
                "SubnetIds": ["subnet-0a1b2c3d", "subnet-1a2b3c4d", "subnet-2a3b4c5d"],
                "VpcId": "vpc-99887766",
                "AppNetworkAccessType": "VpcOnly",
                "AppSecurityGroupManagement": "Service",
                "HomeEfsFileSystemCreation": "Disabled",
                "HomeEfsFileSystemKmsKeyId": "alias/legacy-efs",
                "KmsKeyId": "arn:aws:kms:us-west-2:123456789012:key/abc",
                "TagPropagation": "ENABLED",
                "Tags": [
                    { "Key": "team", "Value": "ml" },
                    { "Key": "cost-centre", "Value": "42" }
                ]
            }),
            expected: vec![
                ("DomainName", json!("vpc-only-studio")),
                ("AppNetworkAccessType", json!("VpcOnly")),
                ("AppSecurityGroupManagement", json!("Service")),
                ("HomeEfsFileSystemCreation", json!("Disabled")),
                ("HomeEfsFileSystemKmsKeyId", json!("alias/legacy-efs")),
                (
                    "KmsKeyId",
                    json!("arn:aws:kms:us-west-2:123456789012:key/abc"),
                ),
                ("TagPropagation", json!("ENABLED")),
                (
                    "SubnetIds",
                    json!(["subnet-0a1b2c3d", "subnet-1a2b3c4d", "subnet-2a3b4c5d"]),
                ),
                (
                    "DefaultUserSettings",
                    json!({
                        "ExecutionRole": EXECUTION_ROLE,
                        "SecurityGroups": ["sg-01", "sg-02"],
                        // The members the service resolves for the domain: Studio is
                        // admitted, so Studio is the landing experience, the home EFS
                        // volume is auto-mounted, and an execution-role session is named
                        // after the user's identity.
                        "StudioWebPortal": "ENABLED",
                        "DefaultLandingUri": "studio::",
                        "AutoMountHomeEFS": "Enabled",
                        "StudioWebPortalSettings": {
                            "ExecutionRoleSessionNameMode": "USER_IDENTITY"
                        }
                    }),
                ),
                ("DomainSettings", json!({ "SecurityGroupIds": ["sg-03"] })),
                (
                    "DefaultSpaceSettings",
                    json!({ "ExecutionRole": EXECUTION_ROLE }),
                ),
            ],
        },
    ];

    let mut minted: Vec<String> = Vec::new();
    for row in &rows {
        let case = row.case;
        let created = ok_in(&emulator, row.region, "CreateDomain", &row.request).await;

        let domain_id = str_field(&created, "DomainId");
        assert!(
            well_formed_domain_id(&domain_id),
            "case {case}: DomainId must match ^d-(-*[a-z0-9]){{1,61}}$, got {domain_id:?}"
        );
        assert!(
            !minted.contains(&domain_id),
            "case {case}: every create mints its own DomainId, got the repeat {domain_id:?}"
        );
        minted.push(domain_id.clone());

        let expected_arn =
            format!("arn:aws:sagemaker:{}:{ACCOUNT}:domain/{domain_id}", row.region);
        assert_eq!(
            str_field(&created, "DomainArn"),
            expected_arn,
            "case {case}: DomainArn, got {created}"
        );
        // The domain's landing address: this emulator's own published base and the domain's
        // own landing path.
        let expected_url = format!(
            "{}/_aws/sagemaker/domain/{domain_id}/studio",
            smithy_server_runtime::UNSERVED_URL_BASE
        );
        assert_eq!(
            str_field(&created, "Url"),
            expected_url,
            "case {case}: Url, got {created}"
        );

        let described = ok_in(
            &emulator,
            row.region,
            "DescribeDomain",
            &json!({ "DomainId": domain_id }),
        )
        .await;
        assert_eq!(
            str_field(&described, "DomainId"),
            domain_id,
            "case {case}: the reader is addressed by the minted id, got {described}"
        );
        assert_eq!(
            str_field(&described, "DomainArn"),
            expected_arn,
            "case {case}: the reader replays the minted ARN, got {described}"
        );
        assert_eq!(
            str_field(&described, "Url"),
            expected_url,
            "case {case}: the reader replays the minted URL, got {described}"
        );
        for (member, expected) in &row.expected {
            assert_eq!(
                described.get(member),
                Some(expected),
                "case {case}: {member}, got {described}"
            );
        }
        // Nothing the emulator does not hold is reported as held: no Amazon EFS file
        // system, no IAM Identity Center application and no VPC security group is created
        // by any service here, so those members are absent rather than naming a resource
        // that resolves to nothing.
        for absent in [
            "HomeEfsFileSystemId",
            "SingleSignOnManagedApplicationInstanceId",
            "SingleSignOnApplicationArn",
            "SecurityGroupIdForDomainBoundary",
            "FailureReason",
        ] {
            assert_eq!(
                described.get(absent),
                None,
                "case {case}: {absent} names nothing this emulator holds, got {described}"
            );
        }
        // A member the request did not carry is reported by neither the create nor the
        // reader: beyond the three members the model documents a default for and the
        // domain's resolved default user settings, nothing is filled in.
        if row.request.get("KmsKeyId").is_none() {
            assert_eq!(
                described.get("KmsKeyId"),
                None,
                "case {case}: an absent KmsKeyId stays absent, got {described}"
            );
        }
    }
}

/// RULE: a member typed by one of the service's enums admits exactly the values that
/// enum declares; a value outside the catalog is refused as a bad request (HTTP 400
/// `ValidationException`) and nothing is stored for it.
#[tokio::test]
async fn create_domain_admits_each_enum_catalog_and_refuses_a_value_outside_it() {
    let emulator = Emulator::new();

    // Every enum-typed member of the create, each with the catalog the model declares
    // for it (read from the generated value slices, not restated here).
    let catalogs: [(&str, &[&str]); 5] = [
        ("AuthMode", AUTH_MODE_VALUES),
        ("AppNetworkAccessType", APP_NETWORK_ACCESS_TYPE_VALUES),
        (
            "AppSecurityGroupManagement",
            APP_SECURITY_GROUP_MANAGEMENT_VALUES,
        ),
        (
            "HomeEfsFileSystemCreation",
            HOME_EFS_FILE_SYSTEM_CREATION_VALUES,
        ),
        ("TagPropagation", TAG_PROPAGATION_VALUES),
    ];

    let mut accepted = 0usize;
    for (member, catalog) in catalogs {
        for value in catalog {
            // One domain per accepted value, under a name of its own: a `DomainName` this
            // account already holds a domain under is refused, which is a different rule
            // from the one under test here.
            let mut request = minimal(&format!("enum-{member}-{value}"));
            request[member] = json!(value);
            let created = emulator.ok("CreateDomain", &request).await;
            let domain_id = str_field(&created, "DomainId");
            let described = emulator
                .ok("DescribeDomain", &json!({ "DomainId": domain_id }))
                .await;
            assert_eq!(
                described.get(member),
                Some(&json!(value)),
                "case {member}={value}: the reader replays the accepted value, got {described}"
            );
            accepted += 1;
        }

        // Outside the catalog: a value of the right type that the enum does not declare,
        // and the right value in the wrong case. Both are bad requests, refused by the
        // modelled enum constraint with the member named as the wire spells it.
        let wire_member = {
            let mut characters = member.chars();
            let first = characters.next().expect("a non-empty member name");
            format!("{}{}", first.to_ascii_lowercase(), characters.as_str())
        };
        let wrong_case = catalog[0].to_ascii_lowercase();
        for outside in ["NotAMember", wrong_case.as_str()] {
            let mut request = minimal(&format!("enum-{member}-outside"));
            request[member] = json!(outside);
            let (status, parsed) = emulator.call("CreateDomain", &request).await;
            assert_eq!(
                status, 400,
                "case {member}={outside}: the request must be refused, got {status} {parsed}"
            );
            assert_eq!(
                error_code(&parsed),
                "ValidationException",
                "case {member}={outside}: error code, got {parsed}"
            );
            let refusal = message(&parsed);
            assert!(
                refusal.contains(&wire_member),
                "case {member}={outside}: the refusal must name the member, got {parsed}"
            );
            for declared in catalog {
                assert!(
                    refusal.contains(declared),
                    "case {member}={outside}: the refusal must name the value set, \
                     missing {declared}, got {parsed}"
                );
            }
        }
    }

    // Exactly the accepted creates are stored: every refusal above wrote nothing.
    let listed = emulator
        .ok("ListDomains", &json!({ "MaxResults": MAX_PAGE_SIZE }))
        .await;
    assert_eq!(
        listed_ids(&listed).len(),
        accepted,
        "a refused create stores nothing: expected {accepted} domains, got {listed}"
    );
}

/// RULE: a `DomainName` outside the shape `DomainName`'s model constrains it to is
/// refused, and nothing is stored for it.
#[tokio::test]
async fn create_domain_refuses_a_name_outside_the_modelled_shape() {
    let emulator = Emulator::new();

    let too_long = "a".repeat(64);
    // `DomainName` is `len 0..63, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`: one past
    // the published length maximum, and each way the pattern can be broken.
    let refused: Vec<(&str, String)> = vec![
        ("one-past-the-63-character-maximum", too_long),
        ("underscore-outside-the-pattern", "bad_name".to_string()),
        ("leading-hyphen", "-studio".to_string()),
        ("trailing-hyphen", "studio-".to_string()),
        ("dot-outside-the-pattern", "studio.one".to_string()),
        ("space-outside-the-pattern", "studio one".to_string()),
        ("empty-name", String::new()),
    ];
    for (case, name) in &refused {
        let (status, parsed) = emulator.call("CreateDomain", &minimal(name)).await;
        assert_eq!(
            status, 400,
            "case {case}: the name {name:?} must be refused, got {status} {parsed}"
        );
    }

    // And the shapes the pattern admits are accepted, so the refusals above are about the
    // shape and not about the operation.
    for (case, name) in [
        ("alphanumeric", "studio1"),
        ("inner-hyphen", "studio-one"),
        ("inner-double-hyphen", "studio--one"),
        ("digits-only", "2024"),
    ] {
        let created = emulator.ok("CreateDomain", &minimal(name)).await;
        assert!(
            well_formed_domain_id(&str_field(&created, "DomainId")),
            "case {case}: {name:?} is inside the modelled shape, got {created}"
        );
    }

    let listed = emulator.ok("ListDomains", &json!({})).await;
    assert_eq!(
        listed_ids(&listed).len(),
        4,
        "a refused create stores nothing: expected the 4 accepted domains, got {listed}"
    );
}

/// RULE: a domain owes its move to `InService` to the CLOCK, a span after the create that
/// starts its provisioning (`state::DOMAIN_IN_SERVICE_AFTER`) — so the read that immediately
/// follows the create still reports the transient `Pending` through both readers, a client
/// that polls for the condition reads `InService`, and a read never changes what a later
/// caller sees.
#[tokio::test]
async fn domain_status_moves_from_pending_to_in_service_on_the_clock() {
    let emulator = Emulator::new();

    let created = emulator.ok("CreateDomain", &minimal("settling")).await;
    let domain_id = str_field(&created, "DomainId");
    let describe = json!({ "DomainId": domain_id });

    // The create leaves the domain provisioning, so the very next request still reports the
    // transient status — through both readers.
    let pending = emulator.ok("DescribeDomain", &describe).await;
    assert_eq!(
        str_field(&pending, "Status"),
        DOMAIN_STATUS_PENDING,
        "case straight-after-create: DescribeDomain status, got {pending}"
    );
    let listed = emulator.ok("ListDomains", &json!({})).await;
    assert_eq!(
        listed
            .get("Domains")
            .and_then(Value::as_array)
            .map(|domains| domains
                .iter()
                .map(|details| str_field(details, "Status"))
                .collect::<Vec<_>>()),
        Some(vec![DOMAIN_STATUS_PENDING.to_string()]),
        "case straight-after-create: ListDomains status, got {listed}"
    );

    // The move out of that status is the clock's and no reader decides it: a domain whose due
    // instant is pushed ahead is reported `Pending` however often it is read.
    {
        let store = emulator
            .engine
            .state::<ComAmazonawsSagemakerSagemakerState>();
        let mut store = store.lock();
        let domain = store
            .domains
            .get_mut(&domain_id)
            .unwrap_or_else(|| panic!("the created domain {domain_id} must be stored"));
        domain.status = DOMAIN_STATUS_PENDING.to_string();
        domain.in_service_due_at = SystemTime::now() + SETTLE_DEADLINE;
    }
    let held = emulator.ok("DescribeDomain", &describe).await;
    assert_eq!(
        str_field(&held, "Status"),
        DOMAIN_STATUS_PENDING,
        "case transition-still-owed: a domain whose move is not due yet keeps the \
         transient status, got {held}"
    );
    {
        let store = emulator
            .engine
            .state::<ComAmazonawsSagemakerSagemakerState>();
        let mut store = store.lock();
        let domain = store
            .domains
            .get_mut(&domain_id)
            .unwrap_or_else(|| panic!("the created domain {domain_id} must be stored"));
        domain.in_service_due_at = SystemTime::now();
    }

    // Wait for the CONDITION, not for a fixed time.
    let deadline = Instant::now() + SETTLE_DEADLINE;
    let settled = loop {
        let described = emulator.ok("DescribeDomain", &describe).await;
        if str_field(&described, "Status") == DOMAIN_STATUS_IN_SERVICE {
            break described;
        }
        assert!(
            Instant::now() < deadline,
            "case clock-owed-move: the domain must reach {DOMAIN_STATUS_IN_SERVICE}, \
             last read {described}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };

    // The transition is the clock's, so reading again reports the same thing: the status
    // does not move a second time and the instant it was due does not drift.
    let again = emulator.ok("DescribeDomain", &describe).await;
    assert_eq!(
        again.get("Status"),
        settled.get("Status"),
        "case repeated-read: the status is stable once settled, got {again}"
    );
    assert_eq!(
        again.get("LastModifiedTime"),
        settled.get("LastModifiedTime"),
        "case repeated-read: LastModifiedTime is stable once settled, got {again}"
    );
    assert_eq!(
        again.get("CreationTime"),
        settled.get("CreationTime"),
        "case repeated-read: CreationTime is stable, got {again}"
    );
    // The listing reports what the reader does: one domain, one status.
    let listed = emulator.ok("ListDomains", &json!({})).await;
    assert_eq!(
        listed
            .get("Domains")
            .and_then(Value::as_array)
            .and_then(|domains| domains.first())
            .and_then(|details| details.get("Status")),
        settled.get("Status"),
        "case settled: ListDomains reports the reader's status, got {listed}"
    );
}

/// RULE: `ListDomains` pages over every stored domain exactly once in one stable order,
/// `MaxResults` bounds the page (defaulting to 10) and a `NextToken` is returned only
/// while a further page exists.
#[tokio::test]
async fn list_domains_pages_every_stored_domain_exactly_once() {
    let emulator = Emulator::new();

    // An account that has created nothing lists nothing — the EMPTY list, not a missing
    // member, and no token.
    let empty = emulator.ok("ListDomains", &json!({})).await;
    assert_eq!(
        empty.get("Domains"),
        Some(&json!([])),
        "case no-domains: the listing reports the empty list, got {empty}"
    );
    assert_eq!(
        empty.get("NextToken"),
        None,
        "case no-domains: no further page, got {empty}"
    );

    // More domains than the default page holds, so the default is observable.
    let seeded = usize::try_from(DOMAIN_DEFAULT_PAGE_SIZE).expect("a positive page size") + 3;
    let mut created: Vec<String> = Vec::new();
    for index in 0..seeded {
        let response = emulator
            .ok("CreateDomain", &minimal(&format!("studio{index}")))
            .await;
        created.push(str_field(&response, "DomainId"));
    }

    let whole = emulator
        .ok("ListDomains", &json!({ "MaxResults": MAX_PAGE_SIZE }))
        .await;
    let order = listed_ids(&whole);
    assert_eq!(
        order.len(),
        seeded,
        "case one-page: every created domain is listed, got {whole}"
    );
    let mut sorted = order.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        seeded,
        "case one-page: no domain is listed twice, got {order:?}"
    );
    let mut expected = created.clone();
    expected.sort();
    assert_eq!(
        sorted, expected,
        "case one-page: the listing reports exactly the created domains, got {order:?}"
    );

    // Rows: the documented default page, each page size from one entry up to past the
    // stored count, and the published `MaxResults` bounds (1 and 100).
    let mut page_sizes: Vec<Option<i64>> = vec![None, Some(1), Some(100)];
    for size in 1..=(seeded + 2) {
        page_sizes.push(Some(size as i64));
    }
    for size in page_sizes {
        let case = match size {
            None => "max-results-absent".to_string(),
            Some(size) => format!("max-results-{size}"),
        };
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut request = json!({});
            if let Some(size) = size {
                request["MaxResults"] = json!(size);
            }
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let page = emulator.ok("ListDomains", &request).await;
            let ids = listed_ids(&page);
            let bound = size
                .unwrap_or(i64::from(DOMAIN_DEFAULT_PAGE_SIZE))
                .min(i64::from(MAX_PAGE_SIZE)) as usize;
            assert!(
                ids.len() <= bound,
                "case {case}: a page holds at most {bound} entries, got {page}"
            );
            walked.extend(ids);
            pages += 1;
            assert!(
                pages <= seeded + 2,
                "case {case}: the walk must terminate, walked {walked:?}"
            );
            match page.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, order,
            "case {case}: the walk reports every domain exactly once, in the listing's \
             one order"
        );
    }
}

/// RULE: a request a reader cannot resolve is refused with the error its model declares
/// and the sentence the service words that fact with, and a page token that names no
/// boundary of the result set is refused rather than silently restarting the listing.
#[tokio::test]
async fn domain_readers_refuse_what_they_cannot_resolve() {
    let emulator = Emulator::new();

    let created = emulator.ok("CreateDomain", &minimal("present")).await;
    let present = str_field(&created, "DomainId");

    // Rows: well-formed ids nothing is stored under, each resolved to the ARN the
    // requested id names in the Region the caller signed for.
    for (case, region, domain_id) in [
        ("absent-id", "us-east-1", "d-000000000000"),
        ("another-absent-id", "us-east-1", "d-abc123def456"),
        ("absent-id-in-another-region", "eu-west-1", "d-0a1b2c3d4e5f"),
    ] {
        let (status, parsed) = emulator
            .call_in(region, "DescribeDomain", &json!({ "DomainId": domain_id }))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: the reader must refuse, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFound",
            "case {case}: error code, got {parsed}"
        );
        let expected = format!(
            "Domain [arn:aws:sagemaker:{region}:{ACCOUNT}:domain/{domain_id}] does not exist."
        );
        assert_eq!(
            message(&parsed),
            expected,
            "case {case}: refusal message, got {parsed}"
        );
    }

    // A `DomainId` outside the modelled pattern is refused before the reader runs.
    for (case, domain_id) in [
        ("no-d-prefix", "studio"),
        ("upper-case", "d-ABC123DEF456"),
        ("underscore", "d-abc_123"),
    ] {
        let (status, parsed) = emulator
            .call("DescribeDomain", &json!({ "DomainId": domain_id }))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: {domain_id:?} is outside the modelled pattern, got {status} {parsed}"
        );
    }

    // A page token naming no boundary of this result set, and one past each published
    // bound of `MaxResults` (range 1..100).
    let refused: Vec<(&str, Value)> = vec![
        ("next-token-not-minted-here", json!({ "NextToken": "nope" })),
        (
            "next-token-naming-no-listed-domain",
            json!({ "NextToken": hex::encode("d-000000000000") }),
        ),
        ("max-results-below-the-minimum", json!({ "MaxResults": 0 })),
        (
            "max-results-one-past-the-maximum",
            json!({ "MaxResults": 101 }),
        ),
    ];
    for (case, request) in refused {
        let (status, parsed) = emulator.call("ListDomains", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: the listing must refuse, got {status} {parsed}"
        );
    }

    // Every refusal above left the store intact.
    let listed = emulator.ok("ListDomains", &json!({})).await;
    assert_eq!(
        listed_ids(&listed),
        vec![present],
        "the refusals changed nothing a later caller sees, got {listed}"
    );
}
