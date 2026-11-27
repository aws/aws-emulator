// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for what a Studio DOMAIN resolves at create time and reports back:
//! the network the create must name, the default user settings the service resolves for the
//! domain, and the Studio address the domain publishes as its `Url`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, on the
//! shared `Emulator` harness in `inference_deployment_lifecycle`, and the published `Url` is
//! fetched the way a browser fetches it (`Emulator::fetch`).
//!
//! Three RULES, each asserted over a TABLE of inputs rather than one recorded call:
//!
//! * a SageMaker AI domain's traffic goes through a VPC and that VPC's subnets, so a create
//!   that names no `VpcId` or no `SubnetIds` resolves no network for the domain and is
//!   refused before any domain exists;
//! * a reader reports the configuration the domain CARRIES, so the members the service
//!   resolves for the domain's `DefaultUserSettings` are resolved once at create time and
//!   replayed by `DescribeDomain`, while every value the request carried is kept as sent;
//! * an address this service publishes for a caller to FOLLOW — a domain's `Url`, and the
//!   `Url` of a space under it — is addressed on the endpoint the request arrived on and
//!   SERVED there, never on a host of AWS's own, and it serves Studio only to a fetch that
//!   has SIGNED IN with the grant `CreatePresignedDomainUrl` mints.
//!
//! In this harness the router is not served on a port, so the base it publishes is
//! `smithy_server_runtime::UNSERVED_URL_BASE` — the same value `ctx.endpoint()` answers
//! in-process, where a served emulator answers its data port's base.

use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::state::{
    ComAmazonawsSagemakerSagemakerState, DOMAIN_STATUS_PENDING,
};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator};

/// An execution role inside `RoleArn`'s modelled pattern, for the `DefaultUserSettings`
/// every create must carry.
const EXECUTION_ROLE: &str = "arn:aws:iam::123456789012:role/service-role/StudioRole";

/// The whole sentence AWS refuses a domain whose networking the request did not name with.
/// A literal on purpose: this suite is what pins the wording, so it does not ask the
/// implementation what it is. It interpolates nothing from the request.
const NETWORK_REQUIRED: &str = "VpcId and SubnetIds are required for SageMaker AI Domain \
                                not created by SageMaker Unified Studio.";

/// How long a poll gives the clock to settle a domain: far longer than the span the state
/// gives a domain's provisioning, so a loaded host still settles well inside it. Only the
/// loop's CONDITION decides when it stops.
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// How often a poll looks again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// A `UserProfileName` every domain that is signed into below holds, so a landing fetch has a
/// grant to present.
const SIGN_IN_PROFILE: &str = "lander";

/// Sign in to the domain `domain_id` of `region`: hold a user profile in it, mint the
/// presigned domain URL `CreatePresignedDomainUrl` answers for that profile, and answer the
/// QUERY that URL carries — the grant token, exactly as the hand-off brings it to a landing
/// address ([`signed_in_fetch`]).
///
/// A landing address serves Studio only to a signed-in session, so every case that expects a
/// landing goes through here rather than fetching anonymously.
async fn sign_in(emulator: &Emulator, region: &str, domain_id: &str) -> String {
    let (status, profile) = emulator
        .call_in(
            region,
            "CreateUserProfile",
            &json!({ "DomainId": domain_id, "UserProfileName": SIGN_IN_PROFILE }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case sign-in-{domain_id}: CreateUserProfile must be served, got {status} with {profile}"
    );
    let (status, minted) = emulator
        .call_in(
            region,
            "CreatePresignedDomainUrl",
            &json!({ "DomainId": domain_id, "UserProfileName": SIGN_IN_PROFILE }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case sign-in-{domain_id}: CreatePresignedDomainUrl must be served, got {status} with \
         {minted}"
    );
    let url = str_field(&minted, "AuthorizedUrl");
    url.split_once('?')
        .map(|(_address, query)| query.to_string())
        .unwrap_or_else(|| panic!("case sign-in-{domain_id}: the minted URL carries a grant"))
}

/// Fetch the published address `url` the way a holder that FOLLOWED a presigned domain URL
/// reaches it: with the grant `query` that hand-off carries.
async fn signed_in_fetch(emulator: &Emulator, url: &str, query: &str) -> (u16, String) {
    emulator.fetch(&format!("{url}?{query}")).await
}

/// The status a domain reports once its provisioning is finished, and the one it reports
/// while the clock still owes that move (`smithy_query(query="shape",shape="DomainStatus")`).
const IN_SERVICE: &str = "InService";
const PENDING: &str = DOMAIN_STATUS_PENDING;

/// Put the stored domain `domain_id` back in the transient `PENDING` status with its move
/// into service held off, and answer nothing.
///
/// A domain's move into service is owed from the instant `CreateDomain` answers
/// (`state::transition_owed_at`), so a case that is about what the domain's address serves
/// BEFORE it is in service seeds that status on the record instead of racing the clock for
/// it. [`give_the_move_back_to_the_clock`] hands it back.
fn hold_off_the_move_into_service(emulator: &Emulator, domain_id: &str) {
    let store = emulator
        .engine
        .state::<ComAmazonawsSagemakerSagemakerState>();
    let mut store = store.lock();
    let domain = store
        .domains
        .get_mut(domain_id)
        .unwrap_or_else(|| panic!("the created domain {domain_id} must be stored"));
    domain.status = PENDING.to_string();
    domain.in_service_due_at = SystemTime::now() + SETTLE_DEADLINE;
}

/// Owe the stored domain `domain_id` its move into service again, so the next reporting
/// call materialises it. The reverse of [`hold_off_the_move_into_service`].
fn give_the_move_back_to_the_clock(emulator: &Emulator, domain_id: &str) {
    let store = emulator
        .engine
        .state::<ComAmazonawsSagemakerSagemakerState>();
    let mut store = store.lock();
    let domain = store
        .domains
        .get_mut(domain_id)
        .unwrap_or_else(|| panic!("the created domain {domain_id} must be stored"));
    domain.in_service_due_at = SystemTime::now();
}

/// A `CreateDomain` request: the three required members, plus `extra`'s members.
fn create_domain(name: &str, extra: &Value) -> Value {
    let mut request = json!({
        "DomainName": name,
        "AuthMode": "IAM",
        "DefaultUserSettings": { "ExecutionRole": EXECUTION_ROLE }
    });
    for (member, value) in extra.as_object().expect("the extra members form an object") {
        request[member] = value.clone();
    }
    request
}

/// The placement members a create that names its network carries.
fn placement() -> Value {
    json!({ "VpcId": "vpc-0a1b2c3d", "SubnetIds": ["subnet-0a1b2c3d"] })
}

/// `count` distinct subnet identifiers in the form Amazon EC2 mints one in, which is what
/// `CreateDomain` admits at `SubnetIds` (`subnet-` and at least eight lowercase
/// alphanumerics, inside `SubnetId`'s modelled `^[-0-9a-zA-Z]+$`, `len 0..32`).
fn subnets(count: usize) -> Value {
    json!((0..count)
        .map(|index| format!("subnet-0a1b2c3d{index:04}"))
        .collect::<Vec<_>>())
}

/// The `DomainId`s a `ListDomains` response lists.
fn listed_ids(body: &Value) -> Vec<String> {
    body.get("Domains")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a Domains list in {body}"))
        .iter()
        .map(|details| str_field(details, "DomainId"))
        .collect()
}

/// Describe `domain_id` until it reports `IN_SERVICE`, or fail past the deadline. Waits for
/// the CONDITION: the transition is the clock's and no call has to make it happen.
async fn wait_for_in_service(emulator: &Emulator, case: &str, domain_id: &str) -> Value {
    let request = json!({ "DomainId": domain_id });
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let described = emulator.ok("DescribeDomain", &request).await;
        if str_field(&described, "Status") == IN_SERVICE {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the domain to reach {IN_SERVICE}, last read {described}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// RULE: a SageMaker AI domain communicates through a VPC and that VPC's subnets, and both
/// are the caller's to name — `VpcId` and `SubnetIds` are optional only for a domain created
/// from SageMaker Unified Studio — so a create that leaves either out is refused with HTTP
/// 400 `ValidationException` and nothing is stored for it, while a create that names both is
/// served.
#[tokio::test]
async fn create_domain_refuses_a_request_that_names_no_network() {
    let emulator = Emulator::new();

    // Rows differ along WHICH half of the placement is missing and along what else the
    // request carries: neither member, each one alone, the subnet list at the maximum its
    // modelled `length 1..16` admits, and a VPC-only network access type (the one whose
    // traffic is entirely the VPC's).
    let refused: Vec<(&str, Value)> = vec![
        ("neither-vpc-nor-subnets", json!({})),
        ("vpc-without-subnets", json!({ "VpcId": "vpc-0a1b2c3d" })),
        ("subnets-without-a-vpc", json!({ "SubnetIds": subnets(1) })),
        (
            "sixteen-subnets-without-a-vpc",
            json!({ "SubnetIds": subnets(16) }),
        ),
        (
            "vpc-without-subnets-in-vpc-only-mode",
            json!({ "VpcId": "vpc-0a1b2c3d", "AppNetworkAccessType": "VpcOnly" }),
        ),
        (
            "neither-member-with-every-other-optional-one",
            json!({
                "AppNetworkAccessType": "PublicInternetOnly",
                "HomeEfsFileSystemCreation": "Enabled",
                "TagPropagation": "ENABLED",
                "Tags": [{ "Key": "team", "Value": "ml" }]
            }),
        ),
    ];
    for (case, extra) in &refused {
        let (status, parsed) = emulator
            .call("CreateDomain", &create_domain(case, extra))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create to be refused, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {parsed}"
        );
        assert_eq!(
            message(&parsed),
            NETWORK_REQUIRED,
            "case {case:?}: expected the refusal {NETWORK_REQUIRED:?}, got {parsed}"
        );
        assert_eq!(
            parsed.get("Url"),
            None,
            "case {case:?}: a refused create publishes no Studio Url, got {parsed}"
        );
    }

    // And a create that names both halves is served, for the placement shapes the model
    // admits: one subnet, the sixteen-subnet maximum, and a `VpcId` at its 32-character
    // length maximum.
    let longest_vpc = format!("vpc-{}", "a".repeat(28));
    let served: Vec<(&str, Value)> = vec![
        ("one-subnet", placement()),
        (
            "sixteen-subnets",
            json!({ "VpcId": "vpc-0a1b2c3d", "SubnetIds": subnets(16) }),
        ),
        (
            "longest-vpc-id",
            json!({ "VpcId": longest_vpc.clone(), "SubnetIds": subnets(1) }),
        ),
    ];
    for (case, extra) in &served {
        let created = emulator
            .ok("CreateDomain", &create_domain(&format!("ok-{case}"), extra))
            .await;
        assert!(
            str_field(&created, "DomainId").starts_with("d-"),
            "case {case:?}: expected a minted DomainId, got {created}"
        );
    }

    // Exactly the served creates are stored: every refusal above wrote nothing.
    let listed = emulator.ok("ListDomains", &json!({})).await;
    assert_eq!(
        listed_ids(&listed).len(),
        served.len(),
        "expected only the {} served creates to be stored, got {listed}",
        served.len()
    );
}

/// One row of the resolved-settings table: the `DefaultUserSettings` the create carries,
/// whatever else it carries, and the members the reader must report under
/// `DefaultUserSettings`.
struct SettingsRow {
    case: &'static str,
    user_settings: Value,
    extra: Value,
    /// `(member, expected JSON)` pairs of `DefaultUserSettings` the reader must report.
    expected: Vec<(&'static str, Value)>,
}

/// RULE: a reader of a domain reports the configuration the domain CARRIES, so the members
/// SageMaker resolves for a domain's default user settings — whether Studio is admitted,
/// which experience a user lands in, whether the home EFS volume is auto-mounted and how an
/// execution-role session is named — are resolved once at create time and replayed by
/// `DescribeDomain`, while every value the request itself carried is kept exactly as sent.
#[tokio::test]
async fn describe_domain_reports_the_resolved_default_user_settings() {
    let emulator = Emulator::new();

    // Rows differ along what the create leaves to the service and what it decides itself:
    // nothing but the role, Studio withheld (which moves the landing experience to Studio
    // Classic's), each resolved member given explicitly, a domain created without a home
    // EFS file system, and portal settings that carry another member but not the
    // session-name mode.
    let rows: Vec<SettingsRow> = vec![
        SettingsRow {
            case: "only-the-execution-role",
            user_settings: json!({ "ExecutionRole": EXECUTION_ROLE }),
            extra: json!({}),
            expected: vec![
                ("ExecutionRole", json!(EXECUTION_ROLE)),
                ("StudioWebPortal", json!("ENABLED")),
                ("DefaultLandingUri", json!("studio::")),
                ("AutoMountHomeEFS", json!("Enabled")),
                (
                    "StudioWebPortalSettings",
                    json!({ "ExecutionRoleSessionNameMode": "USER_IDENTITY" }),
                ),
            ],
        },
        SettingsRow {
            case: "studio-withheld",
            user_settings: json!({
                "ExecutionRole": EXECUTION_ROLE,
                "StudioWebPortal": "DISABLED"
            }),
            extra: json!({}),
            expected: vec![
                ("StudioWebPortal", json!("DISABLED")),
                // `studio::` may only be the landing experience of a domain that admits
                // Studio, so the experience left is Studio Classic's.
                ("DefaultLandingUri", json!("app:JupyterServer:")),
                ("AutoMountHomeEFS", json!("Enabled")),
            ],
        },
        SettingsRow {
            case: "landing-experience-chosen-by-the-request",
            user_settings: json!({
                "ExecutionRole": EXECUTION_ROLE,
                "DefaultLandingUri": "app:JupyterServer:"
            }),
            extra: json!({}),
            expected: vec![
                ("DefaultLandingUri", json!("app:JupyterServer:")),
                ("StudioWebPortal", json!("ENABLED")),
            ],
        },
        SettingsRow {
            case: "auto-mount-withheld-by-the-request",
            user_settings: json!({
                "ExecutionRole": EXECUTION_ROLE,
                "AutoMountHomeEFS": "Disabled"
            }),
            extra: json!({}),
            expected: vec![("AutoMountHomeEFS", json!("Disabled"))],
        },
        SettingsRow {
            case: "no-home-efs-file-system-asked-for",
            user_settings: json!({ "ExecutionRole": EXECUTION_ROLE }),
            extra: json!({ "HomeEfsFileSystemCreation": "Disabled" }),
            // The resolved auto-mount is the domain's own setting: it does not follow what
            // the create asked about the file system itself.
            expected: vec![
                ("AutoMountHomeEFS", json!("Enabled")),
                ("DefaultLandingUri", json!("studio::")),
            ],
        },
        SettingsRow {
            case: "session-name-mode-chosen-by-the-request",
            user_settings: json!({
                "ExecutionRole": EXECUTION_ROLE,
                "StudioWebPortalSettings": { "ExecutionRoleSessionNameMode": "STATIC" }
            }),
            extra: json!({}),
            expected: vec![(
                "StudioWebPortalSettings",
                json!({ "ExecutionRoleSessionNameMode": "STATIC" }),
            )],
        },
        SettingsRow {
            case: "portal-settings-without-the-session-name-mode",
            // `HiddenMlTools` takes members of `MlTools`
            // (`smithy_query(query="shape",shape="MlTools")`), of which `AutoMl` is one.
            user_settings: json!({
                "ExecutionRole": EXECUTION_ROLE,
                "StudioWebPortalSettings": { "HiddenMlTools": ["AutoMl"] }
            }),
            extra: json!({}),
            expected: vec![(
                "StudioWebPortalSettings",
                json!({
                    "HiddenMlTools": ["AutoMl"],
                    "ExecutionRoleSessionNameMode": "USER_IDENTITY"
                }),
            )],
        },
    ];

    for row in &rows {
        let case = row.case;
        let mut extra = row.extra.clone();
        extra["DefaultUserSettings"] = row.user_settings.clone();
        for (member, value) in placement().as_object().expect("an object") {
            extra[member] = value.clone();
        }
        let created = emulator
            .ok("CreateDomain", &create_domain(case, &extra))
            .await;
        let domain_id = str_field(&created, "DomainId");
        let described = emulator
            .ok("DescribeDomain", &json!({ "DomainId": domain_id }))
            .await;

        let settings = described
            .get("DefaultUserSettings")
            .unwrap_or_else(|| panic!("case {case:?}: the reader reports the settings, got {described}"));
        for (member, expected) in &row.expected {
            assert_eq!(
                settings.get(member),
                Some(expected),
                "case {case:?}: expected DefaultUserSettings.{member} {expected}, got {settings}"
            );
        }
        // The domain's network access is reported under the member the model names for it,
        // resolved to the documented default where the request named none.
        assert_eq!(
            described.get("AppNetworkAccessType"),
            Some(&json!("PublicInternetOnly")),
            "case {case:?}: expected the resolved AppNetworkAccessType, got {described}"
        );
        // The settings are resolved ONCE, on the write path: reading again reports the same
        // configuration rather than resolving it a second time.
        let again = emulator
            .ok("DescribeDomain", &json!({ "DomainId": domain_id }))
            .await;
        assert_eq!(
            again.get("DefaultUserSettings"),
            described.get("DefaultUserSettings"),
            "case {case:?}: the resolved settings are stable across reads, got {again}"
        );
    }
}

/// RULE: the `Url` a domain publishes is an address this emulator SERVES — the endpoint the
/// request arrived on plus that domain's own landing path, never a host of AWS's own — it is
/// minted once so every reader replays it, and it serves Studio only to a fetch that signed in
/// with the grant `CreatePresignedDomainUrl` mints: the domain's landing while it is serving
/// Studio, a refusal while it is not, and nothing at all to a fetch that signed in with
/// nothing.
#[tokio::test]
async fn a_domains_url_addresses_a_landing_this_emulator_serves() {
    let emulator = Emulator::new();

    // Rows differ along what the rule generalises over: the Region the create is signed for
    // (the address is the endpoint's, not the Region's), the `DomainName` shape, the
    // authentication mode, and whether the domain admits Studio at all.
    let rows: Vec<(&str, &str, Value)> = vec![
        ("iam-domain", "us-east-1", json!({})),
        (
            "sso-domain-in-another-region",
            "eu-west-2",
            json!({ "AuthMode": "SSO" }),
        ),
        (
            "studio-withheld",
            "ap-southeast-2",
            json!({
                "DefaultUserSettings": {
                    "ExecutionRole": EXECUTION_ROLE,
                    "StudioWebPortal": "DISABLED"
                }
            }),
        ),
    ];

    // Every create is served first, so the whole table waits out ONE provisioning span
    // rather than one each.
    let mut created: Vec<(&str, &str, String, String, bool)> = Vec::new();
    for (case, region, extra) in &rows {
        let mut extra = (*extra).clone();
        for (member, value) in placement().as_object().expect("an object") {
            extra[member] = value.clone();
        }
        let (status, answered) = emulator
            .call_in(region, "CreateDomain", &create_domain(case, &extra))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateDomain, got {status} with {answered}"
        );
        let domain_id = str_field(&answered, "DomainId");
        let url = str_field(&answered, "Url");

        // The address: this emulator's own published base and the domain's landing path,
        // with nothing of AWS's own naming in it.
        let expected = format!(
            "{}/_aws/sagemaker/domain/{domain_id}/studio",
            smithy_server_runtime::UNSERVED_URL_BASE
        );
        assert_eq!(
            url, expected,
            "case {case:?}: expected the Url {expected:?}, got {url:?}"
        );
        assert!(
            !url.contains("sagemaker.aws") && !url.contains("amazonaws.com"),
            "case {case:?}: a published Url must name no host AWS serves, got {url:?}"
        );

        // Both readers replay the one minted address.
        let described = emulator
            .call_in(region, "DescribeDomain", &json!({ "DomainId": domain_id }))
            .await
            .1;
        assert_eq!(
            str_field(&described, "Url"),
            url,
            "case {case:?}: the reader replays the minted Url, got {described}"
        );
        let listed = emulator
            .call_in(region, "ListDomains", &json!({}))
            .await
            .1;
        let listed_url = listed
            .get("Domains")
            .and_then(Value::as_array)
            .and_then(|domains| {
                domains
                    .iter()
                    .find(|details| str_field(details, "DomainId") == domain_id)
            })
            .map(|details| str_field(details, "Url"));
        assert_eq!(
            listed_url.as_deref(),
            Some(url.as_str()),
            "case {case:?}: the listing reports the minted Url, got {listed}"
        );

        // A fetch that signed in with NOTHING learns nothing: Studio is reached by signing
        // in, so the address answers the sign-in reason and reports no member of the domain —
        // whatever status or settings that domain carries.
        let (status, body) = emulator.fetch(&url).await;
        assert_eq!(
            status, 403,
            "case {case:?}: an address fetched with no grant serves no Studio, got {status} \
             {body:?}"
        );
        for withheld in [domain_id.as_str(), case] {
            assert!(
                !body.contains(withheld),
                "case {case:?}: a fetch that did not sign in must learn nothing about the \
                 domain, got {body:?}"
            );
        }

        let admits_studio = extra
            .get("DefaultUserSettings")
            .and_then(|settings| settings.get("StudioWebPortal"))
            .and_then(Value::as_str)
            .is_none_or(|portal| portal == "ENABLED");
        created.push((*case, *region, domain_id, url, admits_studio));
    }

    for (case, region, domain_id, url, admits_studio) in &created {
        let described = wait_for_in_service(&emulator, case, domain_id).await;
        // A domain whose members authenticate against IAM Identity Center signs nobody in
        // this way ("This operation can only be called when the authentication mode equals
        // IAM"), so there is no grant to reach its landing with and the anonymous answer
        // above is all this address serves.
        if str_field(&described, "AuthMode") != "IAM" {
            continue;
        }
        let grant = sign_in(&emulator, region, domain_id).await;

        // The address serves no Studio while the domain is still being provisioned, and the
        // answer says which status stands in the way — unless the domain does not admit
        // Studio at all, which no status of the domain changes. The move into service is the
        // clock's, so the status that stands in the way is seeded on the record rather than
        // raced for.
        hold_off_the_move_into_service(&emulator, domain_id);
        let (status, body) = signed_in_fetch(&emulator, url, &grant).await;
        if *admits_studio {
            assert_eq!(
                status, 503,
                "case {case:?}: a domain that is not serving yet opens no landing, got \
                 {status} {body:?}"
            );
            assert!(
                body.contains(PENDING),
                "case {case:?}: the answer names the status in the way, got {body:?}"
            );
        } else {
            assert_eq!(
                status, 403,
                "case {case:?}: a domain that does not admit Studio opens no landing, got \
                 {status} {body:?}"
            );
        }
        give_the_move_back_to_the_clock(&emulator, domain_id);
        let described = wait_for_in_service(&emulator, case, domain_id).await;

        let (status, body) = signed_in_fetch(&emulator, url, &grant).await;
        if !*admits_studio {
            // A domain whose settings do not admit Studio serves no Studio to land on,
            // however far its own provisioning got.
            assert_eq!(
                status, 403,
                "case {case:?}: a domain that does not admit Studio opens no landing, got \
                 {status} {body:?}"
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case:?}: a signed-in fetch of the published Url must open the domain's \
             landing, got {status} {body:?}"
        );
        let landing: Value = serde_json::from_str(&body)
            .unwrap_or_else(|error| panic!("case {case:?}: the landing must be JSON, got {body:?}: {error}"));
        // Everything the landing reports is read off the domain the control plane reports:
        // the two cannot disagree.
        for member in [
            "DomainId",
            "DomainName",
            "Status",
            "AuthMode",
            "DefaultLandingUri",
        ] {
            let expected = match member {
                "DefaultLandingUri" => described
                    .get("DefaultUserSettings")
                    .and_then(|settings| settings.get(member))
                    .cloned(),
                _ => described.get(member).cloned(),
            };
            assert_eq!(
                landing.get(member).cloned(),
                expected,
                "case {case:?}: the landing reports the domain's own {member}, got {landing}"
            );
        }
        // And WHO it signed in: the profile the grant belongs to.
        assert!(
            landing
                .get("ResourceArn")
                .and_then(Value::as_str)
                .is_some_and(|arn| arn.ends_with(&format!(
                    "user-profile/{domain_id}/{SIGN_IN_PROFILE}"
                ))),
            "case {case:?}: the landing names the profile it signed in, got {landing}"
        );
    }

    // An address naming a domain this account does not hold holds no grant either, so it
    // serves the same sign-in reason rather than disclosing that no domain is there.
    let (status, body) = emulator
        .fetch(&format!(
            "{}/_aws/sagemaker/domain/d-000000000000/studio",
            smithy_server_runtime::UNSERVED_URL_BASE
        ))
        .await;
    assert_eq!(
        status, 403,
        "case \"an address of no domain\": expected 403, got {status} {body:?}"
    );
}

/// RULE: a space is reached THROUGH its domain, so the `Url` a space publishes is its
/// domain's own landing address with the space on it — and this emulator serves that address
/// too, to a fetch that signed in with its domain's grant: it reports the space under its
/// domain once the space is serving, refuses while it is not, lands nowhere for a space the
/// domain does not hold, and serves nothing at all to a fetch that signed in with nothing.
#[tokio::test]
async fn a_spaces_url_addresses_a_landing_this_emulator_serves() {
    let emulator = Emulator::new();

    let created = emulator
        .ok("CreateDomain", &create_domain("spaces", &placement()))
        .await;
    let domain_id = str_field(&created, "DomainId");
    let domain_url = str_field(&created, "Url");
    // A domain admits members only once its own provisioning has settled, which is the
    // clock's: wait for the CONDITION.
    wait_for_in_service(&emulator, "spaces", &domain_id).await;
    // The grant every fetch below presents: a space is reached through its domain, so the
    // domain's own sign-in is what reaches the addresses under it.
    let grant = sign_in(&emulator, "us-east-1", &domain_id).await;

    // Rows differ along the `SpaceName` shape the address carries: one character, a
    // hyphenated name, and the 63-character maximum `SpaceName`'s length admits.
    let longest = format!("s{}", "a".repeat(62));
    let names = ["s", "team-space", longest.as_str()];
    let mut spaces: Vec<(&str, String)> = Vec::new();
    for name in names {
        let request = json!({ "DomainId": domain_id, "SpaceName": name });
        let answered = emulator.ok("CreateSpace", &request).await;
        assert!(
            str_field(&answered, "SpaceArn").contains(&format!("space/{domain_id}/{name}")),
            "case {name:?}: expected the space's own ARN, got {answered}"
        );
        let described = emulator.ok("DescribeSpace", &request).await;
        let url = str_field(&described, "Url");
        let expected = format!("{domain_url}/space/{name}");
        assert_eq!(
            url, expected,
            "case {name:?}: expected the Url {expected:?}, got {url:?}"
        );

        // The address serves nothing while the space is still launching.
        let (status, body) = signed_in_fetch(&emulator, &url, &grant).await;
        assert_eq!(
            status, 503,
            "case {name:?}: a space that is not serving yet opens no landing, got {status} \
             {body:?}"
        );
        // And nothing at all to a fetch that signed in with nothing.
        let (status, body) = emulator.fetch(&url).await;
        assert_eq!(
            status, 403,
            "case {name:?}: an address fetched with no grant serves no Studio, got {status} \
             {body:?}"
        );
        assert!(
            !body.contains(domain_id.as_str()) && !body.starts_with('{'),
            "case {name:?}: a fetch that did not sign in must learn nothing about the space, \
             got {body:?}"
        );
        spaces.push((name, url));
    }

    for (name, url) in &spaces {
        // Wait for the CONDITION the fetch below needs: the space's own move to InService.
        let request = json!({ "DomainId": domain_id, "SpaceName": name });
        let deadline = Instant::now() + SETTLE_DEADLINE;
        loop {
            let described = emulator.ok("DescribeSpace", &request).await;
            if str_field(&described, "Status") == IN_SERVICE {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {name:?}: expected the space to reach {IN_SERVICE}, last read {described}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }

        let (status, body) = signed_in_fetch(&emulator, url, &grant).await;
        assert_eq!(
            status, 200,
            "case {name:?}: a signed-in fetch of the published Url must open the space's \
             landing, got {status} {body:?}"
        );
        let landing: Value = serde_json::from_str(&body).unwrap_or_else(|error| {
            panic!("case {name:?}: the landing must be JSON, got {body:?}: {error}")
        });
        for (member, expected) in [
            ("DomainId", domain_id.as_str()),
            ("SpaceName", name),
            ("SpaceStatus", IN_SERVICE),
        ] {
            assert_eq!(
                landing.get(member).and_then(Value::as_str),
                Some(expected),
                "case {name:?}: the landing reports the space's own {member}, got {landing}"
            );
        }
    }

    // An address naming a space the domain does not hold lands nowhere, for a fetch that did
    // sign in to the domain it names.
    let (status, body) =
        signed_in_fetch(&emulator, &format!("{domain_url}/space/no-such-space"), &grant).await;
    assert_eq!(
        status, 404,
        "case \"an address of no space\": expected 404, got {status} {body:?}"
    );
}
