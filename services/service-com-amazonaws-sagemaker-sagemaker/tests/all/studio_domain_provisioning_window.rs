// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the window a Studio DOMAIN is still being PROVISIONED in, and
//! for the hand-off a presigned domain URL performs when it is followed.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`, `awsJson1_1`,
//! and a plain `GET` for a published address) through the same `MultiServiceRouter` the
//! production router uses, on the shared `Emulator` harness in
//! `inference_deployment_lifecycle`, so what is asserted is what a real client gets.
//!
//! TWO rules are under test, each over a table that spans what it generalises over rather
//! than one recorded flow:
//!
//! * a domain's provisioning is work SageMaker does AFTER `CreateDomain` has answered, so the
//!   domain reports `Pending` to the requests that immediately follow the create and every
//!   create that hangs a member off it in that window is refused HTTP 400
//!   `ValidationException`, while a client that polls for the condition reaches `InService`
//!   and the identical calls are then served. The rows span the member-create being refused
//!   (`CreateUserProfile`, `CreateSpace`) and the sign-in `CreatePresignedDomainUrl`, the
//!   Region signed for and the name shapes;
//! * a presigned domain URL is published on the endpoint this emulator serves and following
//!   it HANDS its holder off to the DOMAIN's own Studio address — the `Url` that domain
//!   publishes — where the session the grant opened is reported. The rows span the Region and
//!   the domain, so two domains of one account are never signed in at one address.
//!
//! Nothing here sleeps for a span or asserts on elapsed time: every wait is a poll for the
//! CONDITION the clock owes, under a generous deadline.

use std::time::Instant;

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::state::{
    DOMAIN_STATUS_IN_SERVICE, DOMAIN_STATUS_PENDING, USER_PROFILE_STATUS_IN_SERVICE,
};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};
use crate::studio_user_profiles::{
    minimal_domain_with_auth, wait_for_domain_in_service, POLL_INTERVAL, SETTLE_DEADLINE,
};

/// The error code every refusal in the provisioning window travels as: `CreateUserProfile`,
/// `CreateSpace` and `CreatePresignedDomainUrl` each model `ResourceInUse` /
/// `ResourceLimitExceeded` / `ResourceNotFound` only
/// (`smithy_query(query="operation", operation="CreateUserProfile,CreateSpace,CreatePresignedDomainUrl")`),
/// so a refusal none of them states is the service-common one.
const VALIDATION: &str = "ValidationException";

/// Which member-create a row drives against the domain that is still provisioning.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Member {
    /// `CreateUserProfile`, refused with the sentence naming the would-be profile's ARN.
    UserProfile,
    /// `CreateSpace`, refused with the same sentence naming the would-be space's ARN.
    Space,
    /// `CreatePresignedDomainUrl`, which creates no member and carries its own sentence.
    PresignedUrl,
}

/// One row of the provisioning-window table.
struct Row {
    case: &'static str,
    region: &'static str,
    /// The `DomainName` the domain is created under.
    domain: &'static str,
    /// The name the member is created under — a `UserProfileName` or a `SpaceName`, both
    /// inside `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`.
    name: &'static str,
    member: Member,
}

/// The ARN `DomainArn`'s modelled pattern gives the domain `domain_id` in `region`.
fn domain_arn(region: &str, domain_id: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:domain/{domain_id}")
}

/// The ARN the member of `domain_id` named `name` resolves to, by the modelled pattern of
/// `UserProfileArn` (`…:user-profile/`) and `SpaceArn` (`…:space/`): the kind, then the
/// domain and the member's own name.
fn member_arn(region: &str, kind: &str, domain_id: &str, name: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:{kind}/{domain_id}/{name}")
}

impl Member {
    /// The operation this member is created by.
    fn target(self) -> &'static str {
        match self {
            Self::UserProfile => "CreateUserProfile",
            Self::Space => "CreateSpace",
            Self::PresignedUrl => "CreatePresignedDomainUrl",
        }
    }

    /// The request body that creates it in `domain_id` under `name`.
    fn request(self, domain_id: &str, name: &str) -> Value {
        match self {
            Self::UserProfile => json!({ "DomainId": domain_id, "UserProfileName": name }),
            Self::Space => json!({ "DomainId": domain_id, "SpaceName": name }),
            Self::PresignedUrl => json!({ "DomainId": domain_id, "UserProfileName": name }),
        }
    }

    /// The whole sentence the call is refused with while the domain is still provisioning.
    /// Spelled out here rather than taken from the service, so the suite pins the wording.
    fn refusal(self, region: &str, domain_id: &str, name: &str) -> String {
        match self {
            Self::UserProfile => format!(
                "Unable to create UserProfile [{}] because Domain [{}] is not in InService.",
                member_arn(region, "user-profile", domain_id, name),
                domain_arn(region, domain_id)
            ),
            Self::Space => format!(
                "Unable to create Space [{}] because Domain [{}] is not in InService.",
                member_arn(region, "space", domain_id, name),
                domain_arn(region, domain_id)
            ),
            Self::PresignedUrl => format!(
                "Domain must be in either InService or Update_Failed state and not in: \
                 {DOMAIN_STATUS_PENDING}"
            ),
        }
    }
}

/// The rows the provisioning-window rule is asserted over: they differ along the member-create
/// being weighed, the Region the calls are signed for, and the name shapes the modelled
/// patterns admit (one character, hyphenated, mixed case, the 63-character maximum).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a user profile of a domain in us-east-1",
            region: "us-east-1",
            domain: "provisioning",
            name: "researcher",
            member: Member::UserProfile,
        },
        Row {
            case: "a single-character space name in another Region",
            region: "eu-west-1",
            domain: "p",
            name: "s",
            member: Member::Space,
        },
        Row {
            case: "a sign-in URL for a domain that is still provisioning",
            region: "ap-southeast-2",
            domain: "signing-in",
            name: "Data-Scientist",
            member: Member::PresignedUrl,
        },
        Row {
            case: "the longest member name the pattern admits",
            region: "us-west-2",
            domain: "longest",
            name: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            member: Member::UserProfile,
        },
    ]
}

/// RULE: a Studio domain is still being provisioned when its create answers, so the requests
/// that immediately follow report `Pending` and every create that hangs a resource off the
/// domain in that window is refused HTTP 400 `ValidationException` with the sentence for that
/// entity — and once the domain reports `InService` the identical calls are served.
#[tokio::test]
async fn a_new_domains_provisioning_window_refuses_the_calls_made_in_it() {
    for row in rows() {
        let Row {
            case,
            region,
            domain,
            name,
            member,
        } = row;
        let emulator = Emulator::new();

        let (status, created) = emulator
            .call_in(region, "CreateDomain", &minimal_domain_with_auth(domain, "IAM"))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateDomain, got {status} with {created}"
        );
        let domain_id = str_field(&created, "DomainId");

        // The very next request, with nothing in between: the domain's provisioning is the
        // clock's and this call lands inside it, so it is refused.
        let (status, refused) = emulator
            .call_in(region, member.target(), &member.request(&domain_id, name))
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, VALIDATION),
            "case {case:?}: {} must be refused while the domain is provisioning, got {status} \
             with {refused}",
            member.target()
        );
        assert_eq!(
            message(&refused),
            member.refusal(region, &domain_id, name),
            "case {case:?}: expected {:?}, got {:?}",
            member.refusal(region, &domain_id, name),
            message(&refused)
        );

        // And the window is OBSERVABLE: the read that follows those calls still reports the
        // provisioning status, so a client that waits for the documented condition has one to
        // wait for.
        let described = emulator
            .call_in(region, "DescribeDomain", &json!({ "DomainId": domain_id }))
            .await
            .1;
        assert_eq!(
            str_field(&described, "Status"),
            DOMAIN_STATUS_PENDING,
            "case {case:?}: the read after those calls must still report the provisioning \
             domain, got {described}"
        );
        // Nothing the refused call touched was stored: the member it would have created is
        // reported by no listing of the domain.
        let listed = emulator
            .call_in(
                region,
                match member {
                    Member::Space => "ListSpaces",
                    _ => "ListUserProfiles",
                },
                &json!({ "DomainIdEquals": domain_id }),
            )
            .await
            .1;
        for held in ["UserProfiles", "Spaces"] {
            assert!(
                listed
                    .get(held)
                    .and_then(Value::as_array)
                    .is_none_or(|members| members.is_empty()),
                "case {case:?}: a refused create stores nothing, got {listed}"
            );
        }

        // Polling for the condition reaches the provisioned domain — on the clock, with no
        // call making it happen.
        wait_for_domain_in_service(&emulator, region, &domain_id).await;

        // The IDENTICAL call is now served. A sign-in URL needs the profile it signs in as, so
        // that profile is created first — itself a call the window refused a moment ago.
        if member == Member::PresignedUrl {
            let (status, profile) = emulator
                .call_in(
                    region,
                    "CreateUserProfile",
                    &json!({ "DomainId": domain_id, "UserProfileName": name }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the profile must be admitted once the domain is \
                 {DOMAIN_STATUS_IN_SERVICE}, got {status} with {profile}"
            );
        }
        let (status, served) = emulator
            .call_in(region, member.target(), &member.request(&domain_id, name))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: {} must be served once the domain is {DOMAIN_STATUS_IN_SERVICE}, \
             got {status} with {served}",
            member.target()
        );
    }
}

/// The address of `url`, i.e. the part a request addresses it by, without its query.
fn address_of(url: &str) -> String {
    url.split('?').next().unwrap_or_default().to_string()
}

/// Create a domain in `region`, wait for the condition that makes it serve, and answer its
/// `DomainId` beside the Studio `Url` it published.
async fn provisioned_domain(emulator: &Emulator, region: &str, name: &str) -> (String, String) {
    let (status, created) = emulator
        .call_in(region, "CreateDomain", &minimal_domain_with_auth(name, "IAM"))
        .await;
    assert_eq!(
        status, 200,
        "case {name}: expected HTTP 200 from CreateDomain, got {status} with {created}"
    );
    let domain_id = str_field(&created, "DomainId");
    let url = str_field(&created, "Url");
    wait_for_domain_in_service(emulator, region, &domain_id).await;
    (domain_id, url)
}

/// Poll `DescribeUserProfile` until the profile serves, so a sign-in is not racing the
/// transition the clock owes it. Waits for the CONDITION, never for a span.
async fn wait_for_profile(emulator: &Emulator, region: &str, domain_id: &str, profile: &str) {
    let read = json!({ "DomainId": domain_id, "UserProfileName": profile });
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, described) = emulator
            .call_in(region, "DescribeUserProfile", &read)
            .await;
        if status == 200 && str_field(&described, "Status") == USER_PROFILE_STATUS_IN_SERVICE {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {profile}: the profile must reach {USER_PROFILE_STATUS_IN_SERVICE} on the \
             clock, last read {status} {described}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// RULE: following a presigned domain URL is a SIGN-IN, so the address it was published at
/// HANDS its holder off — a redirect, not an answer of its own — to the DOMAIN's own Studio
/// address, the `Url` that domain publishes; the session the grant opened is reported there,
/// that address serves it without handing off again, and each domain of the account is signed
/// in at its own address.
#[tokio::test]
async fn following_a_presigned_domain_url_hands_the_holder_off_to_the_domains_studio_address() {
    // Rows differ along the Region signed for and the domain, so the addresses are compared
    // across domains as well as within one.
    let rows: [(&str, &str, &str); 3] = [
        ("the first domain", "us-east-1", "handoff-one"),
        ("a second domain of the same account", "us-east-1", "handoff-two"),
        ("a domain in another Region", "eu-central-1", "handoff-three"),
    ];
    let emulator = Emulator::new();
    let profile = "analyst";
    let mut signed_in: Vec<(&str, String, String, String)> = Vec::new();

    for (case, region, domain) in rows {
        let (domain_id, studio_url) = provisioned_domain(&emulator, region, domain).await;
        let (status, created) = emulator
            .call_in(
                region,
                "CreateUserProfile",
                &json!({ "DomainId": domain_id, "UserProfileName": profile }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateUserProfile must be served, got {status} with {created}"
        );
        wait_for_profile(&emulator, region, &domain_id, profile).await;

        let (status, minted) = emulator
            .call_in(
                region,
                "CreatePresignedDomainUrl",
                &json!({ "DomainId": domain_id, "UserProfileName": profile }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreatePresignedDomainUrl must be served, got {status} with {minted}"
        );
        let url = str_field(&minted, "AuthorizedUrl");

        // The URL leads back to this emulator: never a host AWS publishes its own endpoints
        // or its own Studio at, which resolves to nothing a client of this emulator can
        // reach.
        for forbidden in ["amazonaws.com", "sagemaker.aws", ".api.aws", ".on.aws"] {
            assert!(
                !url.contains(forbidden),
                "case {case:?}: a published URL must name no host AWS serves, got {url:?}"
            );
        }

        // Following it is a HAND-OFF: the sign-in address answers a redirect rather than the
        // session itself.
        let (status, body) = emulator.fetch(&url).await;
        assert_eq!(
            status, 302,
            "case {case:?}: the sign-in address must hand the holder off, got {status} \
             {body:?}"
        );

        // The chain ends at the domain's OWN Studio address — the `Url` that domain
        // publishes — and the session the grant opened is reported there.
        let (status, landed, body) = emulator.follow(case, &url).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the hand-off must end at a served address, got {status} {body:?}"
        );
        assert_eq!(
            address_of(&landed),
            address_of(&studio_url),
            "case {case:?}: the chain must end at the domain's own Studio address, got \
             {landed:?}"
        );
        assert_ne!(
            address_of(&landed),
            address_of(&url),
            "case {case:?}: the sign-in address must hand off rather than serve itself, got \
             {landed:?}"
        );
        let session: Value = serde_json::from_str(&body).unwrap_or_else(|error| {
            panic!("case {case:?}: the landing must be JSON, got {body:?}: {error}")
        });
        assert_eq!(
            str_field(&session, "ResourceArn"),
            member_arn(region, "user-profile", &domain_id, profile),
            "case {case:?}: the landing names the profile it signs in as, got {session}"
        );
        assert_eq!(
            str_field(&session, "DomainId"),
            domain_id,
            "case {case:?}: the landing names the domain it lands in, got {session}"
        );

        // That address SERVES the holder: a fetch that has already reached it is not handed
        // off again, so the chain terminates there.
        let (status, again) = emulator.fetch(&landed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the Studio address must serve the holder rather than hand off \
             again, got {status} {again:?}"
        );

        signed_in.push((case, domain_id, address_of(&url), address_of(&landed)));
    }

    // Each domain is signed in at an address of its OWN: no two domains of the account share
    // either address, and a domain's Studio address is not the address its URL was fetched at.
    for (case, domain_id, sign_in, studio) in &signed_in {
        assert_ne!(
            sign_in, studio,
            "case {case:?}: the sign-in address and the Studio address are two addresses, got \
             {sign_in:?}"
        );
        for (other_case, other_id, other_sign_in, other_studio) in &signed_in {
            if other_id == domain_id {
                continue;
            }
            assert_ne!(
                studio, other_studio,
                "case {case:?} against {other_case:?}: two domains must not share a Studio \
                 address, got {studio:?}"
            );
            assert_ne!(
                sign_in, other_sign_in,
                "case {case:?} against {other_case:?}: two domains must not share a sign-in \
                 address, got {sign_in:?}"
            );
        }
    }
}
