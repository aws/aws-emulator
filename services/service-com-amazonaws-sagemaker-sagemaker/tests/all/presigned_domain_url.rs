// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `CreatePresignedDomainUrl`: the Studio sign-in URL one user
//! profile of a Studio domain is handed, and the session that URL opens.
//!
//! The RULE under test: the operation records a grant on the user profile it signs its
//! holder in as, publishes a URL addressing THAT profile on the endpoint the request
//! arrived on — never a host of AWS's own, which resolves to nothing a client of this
//! emulator can reach — following that URL HANDS its holder off to the domain's own Studio
//! address, where the session opens, and it authenticates for exactly as long as the
//! two windows the request asked for, the URL's own `ExpiresInSeconds` (300 by default) and
//! the session's `SessionExpirationDurationInSeconds` (43200 by default). A `DomainId`, a
//! `UserProfileName` or a `SpaceName` this account holds nothing under is refused with
//! `ResourceNotFound`, and a domain that cannot sign anybody in this way — one still coming
//! up, or one whose `AuthMode` is not IAM — with `ValidationException`.
//!
//! Every case goes over the wire through the same `MultiServiceRouter` the production
//! router uses, on the shared `Emulator` harness in `inference_deployment_lifecycle`: the
//! mint as an `awsJson1_1` call, and the published URL followed the way a browser follows it
//! (`Emulator::follow`).
//!
//! The windows are driven by moving the stored grant into the past rather than by waiting
//! for a clock, so nothing here sleeps for a span or asserts on elapsed time.

use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::state::{
    self as sm, ComAmazonawsSagemakerSagemakerState,
};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};
use crate::studio_user_profiles::{
    domain_arn, minimal_domain_with_auth, seed_studio_domain, user_profile_arn,
    wait_for_domain_in_service, POLL_INTERVAL, SETTLE_DEADLINE,
};

/// The error code the model declares for this operation's not-found failure
/// (`smithy_query(query="operation", operation="CreatePresignedDomainUrl")`).
const RESOURCE_NOT_FOUND: &str = "ResourceNotFound";

/// The error code an unmodelled refusal travels as, the way every other refusal this
/// service words itself does.
const VALIDATION: &str = "ValidationException";

/// One row of the mint table: what the request carries beyond the two required members, and
/// the two windows the grant must end up with.
struct Row {
    case: &'static str,
    region: &'static str,
    /// The `UserProfileName` the profile is CREATED under.
    profile: &'static str,
    /// The `UserProfileName` the presign request spells, which `UserProfileName`'s doc makes
    /// case-insensitive ("This value is not case sensitive").
    presign_as: &'static str,
    /// A space of the domain the session lands in, created first when named.
    space: Option<&'static str>,
    /// Members of the request beyond `DomainId` and `UserProfileName`.
    extra: Value,
    /// How long the URL itself must stay valid, i.e. `ExpiresInSeconds` or its default.
    expires_in: u64,
    /// How long the session it opens must last, i.e. `SessionExpirationDurationInSeconds`
    /// or its default.
    session_seconds: u64,
}

/// The rows the rule is asserted over. They differ along what it generalises over: the
/// Region signed for, the name shapes of the domain's members, the spelling the presign
/// names the profile with, whether a space and a landing page are named, and which validity
/// members the request carries — neither (the documented defaults), each published bound of
/// `ExpiresInSeconds` (5..300) and of `SessionExpirationDurationInSeconds` (1800..43200).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no validity members named",
            region: "us-east-1",
            profile: "researcher",
            presign_as: "researcher",
            space: None,
            extra: json!({}),
            // `ExpiresInSeconds`: "This value defaults to 300";
            // `SessionExpirationDurationInSeconds`: "This value defaults to 43200".
            expires_in: 300,
            session_seconds: 43_200,
        },
        Row {
            case: "both lower bounds",
            region: "eu-west-1",
            profile: "a",
            presign_as: "a",
            space: None,
            extra: json!({ "ExpiresInSeconds": 5, "SessionExpirationDurationInSeconds": 1800 }),
            expires_in: 5,
            session_seconds: 1_800,
        },
        Row {
            case: "both upper bounds",
            region: "ap-northeast-1",
            profile: "LongestNameTheModelAdmitsForAStudioUserProfileNameMember0123",
            presign_as: "LongestNameTheModelAdmitsForAStudioUserProfileNameMember0123",
            space: None,
            extra: json!({ "ExpiresInSeconds": 300, "SessionExpirationDurationInSeconds": 43200 }),
            expires_in: 300,
            session_seconds: 43_200,
        },
        Row {
            case: "a space and a landing page",
            region: "us-west-2",
            profile: "data-scientist",
            presign_as: "data-scientist",
            space: Some("team-space"),
            extra: json!({
                "SpaceName": "team-space",
                "LandingUri": "app:JupyterLab:/lab/tree/notebooks",
                "ExpiresInSeconds": 60
            }),
            expires_in: 60,
            session_seconds: 43_200,
        },
        Row {
            case: "the profile named in another case",
            region: "eu-central-1",
            profile: "DataScientist",
            presign_as: "datascientist",
            space: None,
            extra: json!({ "SessionExpirationDurationInSeconds": 7200 }),
            expires_in: 300,
            session_seconds: 7_200,
        },
    ]
}

/// Call `target` in `region` and require a 200, returning the parsed output — the
/// region-aware form of the harness's own `Emulator::ok`, since every ARN a case compares
/// names the Region the call signed for.
async fn ok_in(emulator: &Emulator, region: &str, target: &str, body: &Value) -> Value {
    let (status, parsed) = emulator.call_in(region, target, body).await;
    assert_eq!(
        status, 200,
        "case {target} {body}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// A `DomainId` inside the modelled `DomainId` pattern (`^d-(-*[a-z0-9]){1,61}$`) derived
/// from a case name, so each row's domain is its own.
fn domain_id_for(case: &str) -> String {
    let body: String = case
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect();
    format!("d-{}", body.to_lowercase())
}

/// Seed the domain of `domain_id` already `InService` and create `profile` in it, plus
/// `space` when the row names one. Answers nothing: what the row needs afterwards is the
/// pair it already knows.
async fn prepare(emulator: &Emulator, row: &Row, domain_id: &str) {
    seed_studio_domain(emulator, row.region, domain_id);
    let (status, created) = emulator
        .call_in(
            row.region,
            "CreateUserProfile",
            &json!({ "DomainId": domain_id, "UserProfileName": row.profile }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {}: CreateUserProfile must succeed, got {status} {created}",
        row.case
    );
    if let Some(space) = row.space {
        let (status, created) = emulator
            .call_in(
                row.region,
                "CreateSpace",
                &json!({ "DomainId": domain_id, "SpaceName": space }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {}: CreateSpace must succeed, got {status} {created}",
            row.case
        );
    }
}

/// Mint a presigned domain URL for `row`'s profile in `domain_id`.
async fn presign(emulator: &Emulator, row: &Row, domain_id: &str) -> (u16, Value) {
    let mut body = json!({ "DomainId": domain_id, "UserProfileName": row.presign_as });
    for (member, value) in row.extra.as_object().expect("an object of extra members") {
        body[member] = value.clone();
    }
    emulator
        .call_in(row.region, "CreatePresignedDomainUrl", &body)
        .await
}

/// The URL a successful mint published.
async fn published_url(emulator: &Emulator, row: &Row, domain_id: &str) -> String {
    let (status, minted) = presign(emulator, row, domain_id).await;
    assert_eq!(
        status, 200,
        "case {}: CreatePresignedDomainUrl must succeed, got {status} {minted}",
        row.case
    );
    str_field(&minted, "AuthorizedUrl")
}

/// The grants the stored profile holds, exactly as the handler recorded them in the one
/// shared store every handler reaches.
fn grants(emulator: &Emulator, domain_id: &str, profile: &str) -> Vec<sm::PresignedUrlGrant> {
    let state = emulator
        .engine
        .state::<ComAmazonawsSagemakerSagemakerState>();
    let guard = state.lock();
    guard
        .user_profiles
        .get(&sm::user_profile_key(domain_id, profile))
        .map(|held| held.grants.clone())
        .unwrap_or_default()
}

/// Move every grant of the stored profile `shift` into the past, so a window the mint
/// recorded has elapsed without the test waiting for it. The clock is what closes a window,
/// so a test that drives it this way needs no sleep and no wall-clock assertion.
fn backdate_grants(emulator: &Emulator, domain_id: &str, profile: &str, shift: Duration) {
    let state = emulator
        .engine
        .state::<ComAmazonawsSagemakerSagemakerState>();
    let mut guard = state.lock();
    let held = guard
        .user_profiles
        .get_mut(&sm::user_profile_key(domain_id, profile))
        .expect("the prepared profile is in the store");
    for grant in &mut held.grants {
        grant.issued_at -= shift;
        grant.expires_at -= shift;
        grant.session_expires_at -= shift;
    }
}

/// The address a session of `resource` is published at: the base this emulator publishes,
/// the reserved `_aws` namespace, this service's plane, the profile's own identifier with
/// its one `/` percent-encoded as a path segment must be, and the `auth` segment the URL
/// signs in at.
const SESSION_KIND: &str = "user-profile";

/// See [`SESSION_KIND`].
fn expected_prefix(domain_id: &str, profile: &str) -> String {
    format!(
        "{}/_aws/sagemaker/{SESSION_KIND}/{domain_id}%2F{profile}/auth?authToken=",
        smithy_server_runtime::UNSERVED_URL_BASE
    )
}

/// The address the domain `domain_id` serves its own Studio at, which is where following the
/// sign-in URL must land: the domain's own landing address, which `DescribeDomain` reports as
/// its `Url`.
fn expected_studio_address(domain_id: &str) -> String {
    format!(
        "{}/_aws/sagemaker/domain/{domain_id}/studio",
        smithy_server_runtime::UNSERVED_URL_BASE
    )
}

/// The address of `url` without its query, which is what a landing is compared by.
fn address_of(url: &str) -> String {
    url.split('?').next().unwrap_or_default().to_string()
}

/// The `authToken` a published URL carries, DECODED — the form the grant holds, since the
/// mint percent-encodes the token into the query.
fn token_of(url: &str) -> String {
    let encoded = url
        .split("authToken=")
        .nth(1)
        .and_then(|rest| rest.split('&').next())
        .unwrap_or_default();
    smithy_server_runtime::percent_decode(encoded)
}

/// Poll `DescribeUserProfile` until the profile reports `InService`, so a case that compares
/// two reads is not racing the transition the clock owes the profile. Waits for the
/// CONDITION, never for a fixed span.
async fn wait_for_profile_in_service(
    emulator: &Emulator,
    region: &str,
    domain_id: &str,
    profile: &str,
) {
    let read = json!({ "DomainId": domain_id, "UserProfileName": profile });
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, described) = emulator
            .call_in(region, "DescribeUserProfile", &read)
            .await;
        if status == 200 && str_field(&described, "Status") == sm::USER_PROFILE_STATUS_IN_SERVICE {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {profile}: the profile must reach {} on the clock, last read {status} \
             {described}",
            sm::USER_PROFILE_STATUS_IN_SERVICE
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// RULE: the published URL addresses an endpoint this emulator serves, carries a grant token
/// the mint recorded on the user profile it signs in as, and the two windows that grant
/// holds are the ones the request asked for — the documented defaults when it asked for
/// neither. Fetching it opens the session that grant describes.
#[tokio::test]
async fn a_presigned_domain_url_publishes_the_session_the_request_asked_for() {
    for row in rows() {
        let emulator = Emulator::new();
        let domain_id = domain_id_for(row.case);
        prepare(&emulator, &row, &domain_id).await;
        let url = published_url(&emulator, &row, &domain_id).await;

        // The address: this emulator's own published base and the session path of the
        // profile as it is STORED, with nothing of AWS's own naming in it.
        let prefix = expected_prefix(&domain_id, row.profile);
        assert!(
            url.starts_with(&prefix),
            "case {}: expected a URL starting {prefix:?}, got {url:?}",
            row.case
        );
        assert!(
            !url.contains("sagemaker.aws") && !url.contains("amazonaws.com"),
            "case {}: a published URL must not name a host AWS serves, got {url:?}",
            row.case
        );

        // Where the session lands travels in the URL, and only when the request named it.
        for (member, field) in [("SpaceName", "spaceName"), ("LandingUri", "landingUri")] {
            let named = row.extra.get(member).and_then(Value::as_str);
            let carried = url.contains(&format!("&{field}="));
            assert_eq!(
                carried,
                named.is_some(),
                "case {}: {field} must be carried exactly when {member} is named ({named:?}), \
                 got {url:?}",
                row.case
            );
        }

        // The token: a grant the profile holds, not a value invented for the response.
        let token = token_of(&url);
        let held = grants(&emulator, &domain_id, row.profile);
        let grant = held
            .iter()
            .find(|grant| grant.token == token)
            .unwrap_or_else(|| {
                panic!(
                    "case {}: the token in {url:?} must be a grant the profile holds, got \
                     {held:?}",
                    row.case
                )
            });

        // The two windows the request asked for, as the grant records them.
        let url_window = grant
            .expires_at
            .duration_since(grant.issued_at)
            .expect("a URL window after the instant it was issued");
        assert_eq!(
            url_window,
            Duration::from_secs(row.expires_in),
            "case {}: the URL must stay valid for {}s, got {url_window:?}",
            row.case,
            row.expires_in
        );
        let session_window = grant
            .session_expires_at
            .duration_since(grant.issued_at)
            .expect("a session window after the instant it was issued");
        assert_eq!(
            session_window,
            Duration::from_secs(row.session_seconds),
            "case {}: the session must last {}s, got {session_window:?}",
            row.case,
            row.session_seconds
        );

        // Following it signs the holder in at the DOMAIN's own Studio address — the `Url`
        // that domain publishes, never the sign-in address it was fetched at — and that
        // landing reports the session the grant opened, named by the profile's ARN.
        let (status, landed, body) = emulator.follow(row.case, &url).await;
        assert_eq!(
            status, 200,
            "case {}: following the published URL must open a session, got {status} {body:?}",
            row.case
        );
        assert_eq!(
            address_of(&landed),
            expected_studio_address(&domain_id),
            "case {}: following {url:?} must land at the domain's own Studio address, got \
             {landed:?}",
            row.case
        );
        assert_ne!(
            address_of(&landed),
            address_of(&url),
            "case {}: the sign-in address must hand off rather than serve itself, got \
             {landed:?}",
            row.case
        );
        let session: Value = serde_json::from_str(&body).unwrap_or_else(|error| {
            panic!(
                "case {}: the session must be JSON, got {body:?}: {error}",
                row.case
            )
        });
        assert_eq!(
            str_field(&session, "ResourceArn"),
            user_profile_arn(row.region, &domain_id, row.profile),
            "case {}: the session must name the profile it signs in as, got {session}",
            row.case
        );
        let expected_expiry = grant
            .session_expires_at
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("a session expiry after the epoch")
            .as_secs_f64();
        let reported = session
            .get("SessionExpiresAt")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        // The instant travels as a JSON number, so the comparison allows the rounding a
        // decimal rendering of an f64 introduces — a millisecond, where the windows under
        // test are half an hour and longer.
        assert!(
            (reported - expected_expiry).abs() < 0.001,
            "case {}: the session must end when the grant says ({expected_expiry}), got \
             {reported}",
            row.case
        );
    }
}

/// RULE: the URL authenticates only inside its OWN window: once `ExpiresInSeconds` has
/// elapsed it stops authenticating, even though the session window it opened is still open
/// and the profile still exists.
#[tokio::test]
async fn a_presigned_domain_url_stops_authenticating_once_its_window_elapses() {
    for row in rows() {
        let emulator = Emulator::new();
        let domain_id = domain_id_for(row.case);
        prepare(&emulator, &row, &domain_id).await;
        let url = published_url(&emulator, &row, &domain_id).await;

        // The control: while the grant is live the address hands its holder off to the
        // domain's own Studio address, where the session opens.
        let (status, landed, body) = emulator.follow(row.case, &url).await;
        assert_eq!(
            (status, address_of(&landed)),
            (200, expected_studio_address(&domain_id)),
            "case {}: a fresh URL must sign its holder in, got {status} {landed:?} {body:?}",
            row.case
        );

        // The clock, driven rather than waited for: the URL's own window is now in the
        // past, while the session window the request asked for is not.
        backdate_grants(
            &emulator,
            &domain_id,
            row.profile,
            Duration::from_secs(row.expires_in + 1),
        );
        let (status, landed, body) = emulator.follow(row.case, &url).await;
        assert_eq!(
            status, 403,
            "case {}: a URL past its {}s window must not authenticate, got {status} {landed:?} \
             {body:?}",
            row.case, row.expires_in
        );
    }
}

/// RULE: every URL a profile was handed keeps authenticating while its own window is open,
/// and opening a session is a READ — what a later `DescribeUserProfile` reports is
/// untouched, and the grant token is reported by no read.
#[tokio::test]
async fn every_minted_url_authenticates_and_opening_one_changes_no_later_read() {
    let emulator = Emulator::new();
    let row = Row {
        case: "two urls for one profile",
        region: "us-east-1",
        profile: "analyst",
        presign_as: "analyst",
        space: None,
        extra: json!({}),
        expires_in: 300,
        session_seconds: 43_200,
    };
    let domain_id = domain_id_for(row.case);
    prepare(&emulator, &row, &domain_id).await;
    // The profile's own `Pending` -> `InService` transition is the clock's, so it is settled
    // before the two reads below are compared.
    wait_for_profile_in_service(&emulator, row.region, &domain_id, row.profile).await;

    let first = published_url(&emulator, &row, &domain_id).await;
    let second = published_url(&emulator, &row, &domain_id).await;
    assert_ne!(
        token_of(&first),
        token_of(&second),
        "case {}: each mint must record its own grant, got {first:?} twice",
        row.case
    );

    let read = json!({ "DomainId": domain_id, "UserProfileName": row.profile });
    let before = ok_in(&emulator, row.region, "DescribeUserProfile", &read).await;
    for (attempt, url) in [("first url", &first), ("second url", &second)] {
        for fetch in ["first fetch", "second fetch"] {
            let (status, landed, body) = emulator.follow(row.case, url).await;
            assert_eq!(
                (status, address_of(&landed)),
                (200, expected_studio_address(&domain_id)),
                "case {}/{attempt}/{fetch}: the session must open at the domain's own Studio \
                 address, got {status} {landed:?} {body:?}",
                row.case
            );
        }
        let after = ok_in(&emulator, row.region, "DescribeUserProfile", &read).await;
        assert_eq!(
            after, before,
            "case {}/{attempt}: opening a session must leave what a later reader sees \
             untouched",
            row.case
        );
        assert!(
            !after.to_string().contains(&token_of(url)),
            "case {}/{attempt}: the grant token must be reported by no read, got {after}",
            row.case
        );
    }
}

/// RULE: a presigned domain URL is minted only for a profile of a domain that can sign
/// somebody in — the resources the request names have to resolve (`ResourceNotFound`), and
/// the domain has to be `InService` and authenticate against IAM (`ValidationException`,
/// since the operation models no error for either).
#[tokio::test]
async fn create_presigned_domain_url_refuses_what_it_cannot_sign_in() {
    const REGION: &str = "us-east-1";
    let emulator = Emulator::new();

    // A domain this account holds, with a profile and a space in it, so each row below
    // fails for exactly the one reason it names.
    let live = "d-refusals";
    seed_studio_domain(&emulator, REGION, live);
    ok_in(
        &emulator,
        REGION,
        "CreateUserProfile",
        &json!({ "DomainId": live, "UserProfileName": "held" }),
    )
    .await;

    // A domain the clock has not finished provisioning: its `InService` is pushed far out,
    // so the row weighs the status rather than racing the transition.
    let pending = "d-pending";
    seed_studio_domain(&emulator, REGION, pending);
    {
        let state = emulator
            .engine
            .state::<ComAmazonawsSagemakerSagemakerState>();
        let mut guard = state.lock();
        let domain = guard
            .domains
            .get_mut(pending)
            .expect("the seeded domain is in the store");
        domain.status = sm::DOMAIN_STATUS_PENDING.to_string();
        domain.in_service_due_at = SystemTime::now() + SETTLE_DEADLINE;
    }

    // A domain whose members authenticate against IAM Identity Center, created through the
    // API so its `AuthMode` is the one `CreateDomain` stored.
    let (status, created) = emulator
        .call_in(
            REGION,
            "CreateDomain",
            &minimal_domain_with_auth("sso-domain", "SSO"),
        )
        .await;
    assert_eq!(
        status, 200,
        "case sso: CreateDomain must succeed, got {status} {created}"
    );
    let sso_domain = str_field(&created, "DomainId");
    wait_for_domain_in_service(&emulator, REGION, &sso_domain).await;
    ok_in(
        &emulator,
        REGION,
        "CreateUserProfile",
        &json!({
            "DomainId": sso_domain,
            "UserProfileName": "ssouser",
            "SingleSignOnUserIdentifier": "UserName",
            "SingleSignOnUserValue": "ssouser"
        }),
    )
    .await;

    // Rows differ along WHICH precondition fails, and each names the message the service
    // words that fact with.
    let absent_domain = "d-nosuchdomain";
    let rows: Vec<(&str, Value, &str, String)> = vec![
        (
            "no domain under that id",
            json!({ "DomainId": absent_domain, "UserProfileName": "held" }),
            RESOURCE_NOT_FOUND,
            format!("Domain [{absent_domain}] does not exist"),
        ),
        (
            "the domain holds no such profile",
            json!({ "DomainId": live, "UserProfileName": "absent" }),
            RESOURCE_NOT_FOUND,
            format!(
                "UserProfile [{}] does not exist.",
                user_profile_arn(REGION, live, "absent")
            ),
        ),
        (
            "the domain holds no such space",
            json!({ "DomainId": live, "UserProfileName": "held", "SpaceName": "nospace" }),
            RESOURCE_NOT_FOUND,
            format!(
                "Space [arn:aws:sagemaker:{REGION}:{ACCOUNT}:space/{live}/nospace] does not \
                 exist."
            ),
        ),
        (
            "the domain is still coming up",
            json!({ "DomainId": pending, "UserProfileName": "held" }),
            VALIDATION,
            "Domain must be in either InService or Update_Failed state and not in: Pending"
                .to_string(),
        ),
        (
            "the domain does not authenticate against IAM",
            json!({ "DomainId": sso_domain, "UserProfileName": "ssouser" }),
            VALIDATION,
            format!(
                "Unable to create a presigned domain URL for Domain [{}] because its AuthMode \
                 is SSO instead of IAM.",
                domain_arn(REGION, &sso_domain)
            ),
        ),
    ];

    for (case, request, expected_code, expected_message) in rows {
        let (status, refused) = emulator
            .call_in(REGION, "CreatePresignedDomainUrl", &request)
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400 for {request}, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            expected_code,
            "case {case}: expected {expected_code}, got {refused}"
        );
        assert_eq!(
            message(&refused),
            expected_message,
            "case {case}: expected {expected_message:?}, got {refused}"
        );
    }

    // Nothing a refused call touched was recorded: the profile that does exist holds no
    // grant, so no refusal handed a URL out quietly.
    let held = grants(&emulator, live, "held");
    assert!(
        held.is_empty(),
        "case refusals: a refused mint must record no grant, got {held:?}"
    );
}
