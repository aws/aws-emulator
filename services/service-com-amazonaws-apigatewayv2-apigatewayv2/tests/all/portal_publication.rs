// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ApiGatewayV2 portal publication lifecycle —
//! `PublishPortal`, `DisablePortal` and `PreviewPortal`.
//!
//! The rules these cases assert:
//!
//! * **Each of the three calls only STARTS a build the service then carries out**: it
//!   answers its modelled success status with no body (202 for a publish and a preview,
//!   204 for a disable), the portal reports the `..._IN_PROGRESS` status of that build
//!   until the build's span has passed, and the status it settles in afterwards comes
//!   from the CLOCK — a later read reports it with nothing having been called in
//!   between.
//! * **A publish records the publication it made**: once settled, the portal reports the
//!   instant it published as `LastPublished` and the description the publish request
//!   carried as `LastPublishedDescription` ("When the portal is published, this
//!   description becomes the last published description"), and a republish replaces it.
//! * **A disable takes the publication down but not its history**: the portal reports
//!   `DISABLE_IN_PROGRESS` while the service removes the publication and settles in
//!   `DISABLED`, from where it serves nothing, and still reports the last publication
//!   there WAS. (That the portal's DOMAIN stops serving from the instant the disable is
//!   accepted, rather than when that status settles, is asserted in
//!   `portal_publication_revocation`; that a disable is accepted only from a status a
//!   publication can be taken down from, in `portal_terminal_lifecycle`.)
//! * **A preview changes nothing about what the portal publishes**: its `PublishStatus`
//!   is left exactly as it was, and the portal reports a `Preview` whose status moves
//!   from `PREVIEW_IN_PROGRESS` to `PREVIEW_READY` and whose URL names this portal —
//!   while a portal nothing has previewed reports the EMPTY preview.
//! * **A build starts only when what it needs is free**: a publish and a disable each
//!   need the portal's publication still, a publish and a preview each need its content
//!   build free, and a disable builds no content. So a preview and a disable run side by
//!   side while a publish runs alone. A build that cannot start is refused 409
//!   `ConflictException` and writes nothing, and the refusal lifts of its own accord once
//!   the build the service already owes comes due — unless what the first build left is a
//!   portal publishing nothing, which a second disable has nothing to take down from.
//! * **A portal is built only through its own minted id**: an id nothing minted is 404
//!   `NotFoundException` from all three calls.
//!
//! The rows vary what the rules generalise over: WHICH of the three builds runs first
//! and which runs second (every ordered pair of them), the publication the portal holds
//! when a build starts (one publish settled, two settled, a build still in flight), and
//! the description a publish carries (none, the empty string, a typical one, the maximum
//! 1024 characters the modelled `__stringMin0Max1024` allows, and one replacing an
//! earlier one).

use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::ops::types::{
    PREVIEW_STATUS_VALUES, PUBLISH_STATUS_VALUES,
};
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    PORTAL_BUILD, PREVIEW_STATUS_PREVIEW_IN_PROGRESS, PREVIEW_STATUS_PREVIEW_READY,
    PUBLISH_STATUS_DISABLED, PUBLISH_STATUS_DISABLE_IN_PROGRESS, PUBLISH_STATUS_PUBLISHED,
    PUBLISH_STATUS_PUBLISH_IN_PROGRESS,
};

use crate::api_lifecycle::{as_str, call, carries_nothing, emulator, error_code, instant};
use crate::portal_lifecycle::portal_content;

/// How far a reported instant may sit from this test's own clock reading, in seconds.
/// The service reports a timestamp at whole-second granularity, so a minute of slack
/// pins the instant while surviving a CI scheduling hiccup.
const INSTANT_SLACK_SECONDS: f64 = 60.0;

/// The longest description `PublishPortal$Description` takes: the published upper bound
/// of the modelled `__stringMin0Max1024` (len 0..1024).
const MAX_DESCRIPTION_LEN: usize = 1024;

/// How long to wait so a build the service owes has certainly come due: its span plus
/// slack. A BLOCKING wait on purpose, for the reason given in `portal_product_update`:
/// this workspace's `tokio` carries no `time` feature.
fn past_the_build() -> Duration {
    PORTAL_BUILD + Duration::from_millis(400)
}

/// The three calls that start a server-managed portal build, each with the success
/// status its model declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Build {
    /// `POST /v2/portals/{PortalId}/publish`, modelled 202.
    Publish,
    /// `DELETE /v2/portals/{PortalId}/publish`, modelled 204.
    Disable,
    /// `POST /v2/portals/{PortalId}/preview`, modelled 202.
    Preview,
}

impl Build {
    fn method(self) -> &'static str {
        match self {
            Build::Publish | Build::Preview => "POST",
            Build::Disable => "DELETE",
        }
    }

    fn uri(self, portal_id: &str) -> String {
        match self {
            Build::Publish | Build::Disable => format!("/v2/portals/{portal_id}/publish"),
            Build::Preview => format!("/v2/portals/{portal_id}/preview"),
        }
    }

    /// The status the operation's model declares for a request it accepts.
    fn accepted_status(self) -> u16 {
        match self {
            Build::Publish | Build::Preview => 202,
            Build::Disable => 204,
        }
    }

    /// The status the portal reports while THIS build is in flight: a `PublishStatus`
    /// for a publish or a disable, a `PreviewStatus` for a preview.
    fn in_progress(self) -> &'static str {
        match self {
            Build::Publish => PUBLISH_STATUS_PUBLISH_IN_PROGRESS,
            Build::Disable => PUBLISH_STATUS_DISABLE_IN_PROGRESS,
            Build::Preview => PREVIEW_STATUS_PREVIEW_IN_PROGRESS,
        }
    }

    /// Whether this build needs the portal's publication to be still: a publish and a
    /// disable each move the publication, so neither starts while the other is moving it.
    fn needs_a_still_publication(self) -> bool {
        matches!(self, Build::Publish | Build::Disable)
    }

    /// Whether this build builds the portal's CONTENT — which it both needs free and
    /// occupies.
    fn builds_content(self) -> bool {
        matches!(self, Build::Publish | Build::Preview)
    }

    /// Whether a build already in flight holds THIS one up: it does when this build
    /// needs something that one is using — the publication, or the content build.
    fn is_held_up_by(self, in_flight: Build) -> bool {
        (self.needs_a_still_publication() && in_flight.changes_publication())
            || (self.builds_content() && in_flight.builds_content())
    }

    /// Whether this build changes WHAT the portal publishes, so it occupies the
    /// publication until it comes due.
    fn changes_publication(self) -> bool {
        matches!(self, Build::Publish | Build::Disable)
    }

    /// The status this build's progress is reported under on a portal: a publish and a
    /// disable move the portal's own `PublishStatus`, a preview its `Preview`.
    fn reported_status(self, case: &str, portal: &Value) -> String {
        match self {
            Build::Preview => preview_member(case, portal, "previewStatus"),
            Build::Publish | Build::Disable => publish_status(case, portal),
        }
    }
}

/// Every build, so a table can span the calls rather than name one.
const EVERY_BUILD: [Build; 3] = [Build::Publish, Build::Disable, Build::Preview];

/// Create a portal and answer `(PortalId, PortalDefaultDomainName)` — the id every build
/// addresses it by and the domain name this emulator generates for it.
async fn create_portal(emulator: &MultiServiceRouter, case: &str, display_name: &str) -> (String, String) {
    let request = json!({
        "authorization": { "none": {} },
        "endpointConfiguration": { "none": {} },
        "portalContent": portal_content(display_name)
    });
    let (status, created) = call(emulator, "POST", "/v2/portals", Some(request)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreatePortal, got {status} {created}"
    );
    let default_domain = created
        .get("endpointConfiguration")
        .and_then(|endpoint| endpoint.get("portalDefaultDomainName"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: the create must report a portalDefaultDomainName, got {created}")
        })
        .to_string();
    (as_str(&created, "portalId"), default_domain)
}

/// `GetPortal`, which must answer 200 for a portal the account holds.
async fn get_portal(emulator: &MultiServiceRouter, case: &str, portal_id: &str) -> Value {
    let (status, body) = call(emulator, "GET", &format!("/v2/portals/{portal_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetPortal, got {status} {body}"
    );
    body
}

/// Start `build` on `portal_id` and answer `(status, body)`.
async fn start(
    emulator: &MultiServiceRouter,
    build: Build,
    portal_id: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call(emulator, build.method(), &build.uri(portal_id), body).await
}

/// Start `build` and assert it was ACCEPTED with the status its model declares and no
/// body: the work itself is the service's, so there is nothing to report yet.
async fn accepted(emulator: &MultiServiceRouter, case: &str, build: Build, portal_id: &str, body: Option<Value>) {
    let (status, answered) = start(emulator, build, portal_id, body).await;
    assert_eq!(
        status,
        build.accepted_status(),
        "case {case:?}: expected {} from {build:?}, got {status} {answered}",
        build.accepted_status()
    );
    // The modelled response carries no members — a 204 answers nothing at all and a 202
    // answers the empty structure — so what is asserted is that the body reports no
    // value: the build itself is the service's, with nothing to report yet.
    let reports_nothing = match &answered {
        Value::Null => true,
        Value::Object(members) => members.is_empty(),
        _ => false,
    };
    assert!(
        reports_nothing,
        "case {case:?}: expected no member in the body from {build:?}, got {answered}"
    );
}

/// The `PublishStatus` a portal reports, asserted to be one of the modelled values.
fn publish_status(case: &str, portal: &Value) -> String {
    let status = as_str(portal, "publishStatus");
    assert!(
        PUBLISH_STATUS_VALUES.contains(&status.as_str()),
        "case {case:?}: expected a modelled PublishStatus out of {PUBLISH_STATUS_VALUES:?}, got {portal}"
    );
    status
}

/// A member of the `Preview` a portal reports.
fn preview_member(case: &str, portal: &Value, member: &str) -> String {
    portal
        .get("preview")
        .and_then(|preview| preview.get(member))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the portal must report preview.{member}, got {portal}"))
        .to_string()
}

/// Assert a reported instant is this test's own clock reading, within the slack.
fn assert_instant_now(case: &str, portal: &Value, member: &str) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default();
    let reported = instant(case, portal, member);
    assert!(
        (now - reported).abs() <= INSTANT_SLACK_SECONDS,
        "case {case:?}: expected {member} to be the instant the build settled; got {reported} against a clock reading {now}"
    );
}

/// One publish row: the description the publish under test carries, and whether the
/// portal was already published (and settled) before it.
struct PublishRow {
    case: &'static str,
    description: Option<String>,
    /// The description of the earlier publish, when the row republishes.
    earlier: Option<&'static str>,
}

fn publish_rows() -> Vec<PublishRow> {
    vec![
        PublishRow {
            case: "no description: the published version has none",
            description: Option::None,
            earlier: Option::None,
        },
        PublishRow {
            case: "the empty description, the low end of the modelled 0..1024 length",
            description: Some(String::new()),
            earlier: Option::None,
        },
        PublishRow {
            case: "a typical description",
            description: Some("first cut of the developer portal".to_string()),
            earlier: Option::None,
        },
        PublishRow {
            case: "a description of the maximum 1024 characters",
            description: Some("d".repeat(MAX_DESCRIPTION_LEN)),
            earlier: Option::None,
        },
        PublishRow {
            case: "a republish whose description replaces the earlier one",
            description: Some("second cut, with the billing product included".to_string()),
            earlier: Some("superseded by the republish"),
        },
        PublishRow {
            case: "a republish carrying no description, which is what the last version has",
            description: Option::None,
            earlier: Some("superseded by a version with no description"),
        },
    ]
}

#[tokio::test]
async fn publish_portal_is_server_managed_and_settles_in_published() {
    let emulator = emulator();

    for row in publish_rows() {
        let case = row.case;
        let (portal_id, _) = create_portal(&emulator, case, "Publication Portal").await;

        // A portal that has never been published reports no publication at all.
        let fresh = get_portal(&emulator, case, &portal_id).await;
        assert_eq!(
            publish_status(case, &fresh),
            PUBLISH_STATUS_DISABLED,
            "case {case:?}: a created portal is not serving anything yet, got {fresh}"
        );
        assert!(
            carries_nothing(&fresh, "lastPublished"),
            "case {case:?}: a portal nothing has published reports no lastPublished, got {fresh}"
        );

        if let Some(earlier) = row.earlier {
            accepted(
                &emulator,
                case,
                Build::Publish,
                &portal_id,
                Some(json!({ "description": earlier })),
            )
            .await;
            sleep(past_the_build());
            let settled = get_portal(&emulator, case, &portal_id).await;
            assert_eq!(
                as_str(&settled, "lastPublishedDescription"),
                earlier,
                "case {case:?}: expected the earlier publish to be the last published one, got {settled}"
            );
        }

        let request = row
            .description
            .as_ref()
            .map(|description| json!({ "description": description }));
        accepted(&emulator, case, Build::Publish, &portal_id, request).await;

        // The publish is the SERVICE's work and is not done when the call answers: the
        // portal reports it in progress, and the publication it will make is not yet the
        // one the portal reports.
        let building = get_portal(&emulator, case, &portal_id).await;
        assert_eq!(
            publish_status(case, &building),
            PUBLISH_STATUS_PUBLISH_IN_PROGRESS,
            "case {case:?}: expected the started publish to be in progress, got {building}"
        );
        match row.earlier {
            Some(earlier) => assert_eq!(
                as_str(&building, "lastPublishedDescription"),
                earlier,
                "case {case:?}: expected the in-flight publish to leave the last published \
                 description alone, got {building}"
            ),
            Option::None => assert!(
                carries_nothing(&building, "lastPublished"),
                "case {case:?}: expected an in-flight first publish to report no \
                 lastPublished, got {building}"
            ),
        }

        // The transition is owed by the CLOCK: the same read reports it settled with
        // nothing having been called in between.
        sleep(past_the_build());
        let settled = get_portal(&emulator, case, &portal_id).await;
        assert_eq!(
            publish_status(case, &settled),
            PUBLISH_STATUS_PUBLISHED,
            "case {case:?}: expected the publish to come due and report \
             {PUBLISH_STATUS_PUBLISHED}, got {settled}"
        );
        assert_instant_now(case, &settled, "lastPublished");
        match &row.description {
            Some(description) => assert_eq!(
                settled.get("lastPublishedDescription").and_then(Value::as_str),
                Some(description.as_str()),
                "case {case:?}: expected the publish's description to become the last \
                 published one, got {settled}"
            ),
            Option::None => assert!(
                carries_nothing(&settled, "lastPublishedDescription"),
                "case {case:?}: expected a publish carrying no description to leave the \
                 last published version without one, got {settled}"
            ),
        }
    }
}

/// One disable row: the publication the portal holds when the disable arrives, named by
/// the descriptions of the publishes that made it.
struct DisableRow {
    case: &'static str,
    /// The description of the publish SETTLED before the disable: every row has one,
    /// because a publication is what a disable takes down
    /// (`PORTAL_DISABLE_FROM_STATUSES`).
    published_description: &'static str,
    /// The description of a second settled publish, when the row republishes before the
    /// disable.
    republished_description: Option<&'static str>,
}

#[tokio::test]
async fn disable_portal_takes_the_publication_down_and_keeps_its_history() {
    let emulator = emulator();

    for row in [
        DisableRow {
            case: "a published portal is taken back down",
            published_description: "the version being taken down",
            republished_description: Option::None,
        },
        DisableRow {
            case: "a republished portal is taken down, keeping the last version's \
                   description",
            published_description: "the first version",
            republished_description: Some("the version being taken down"),
        },
    ] {
        let case = row.case;
        let (portal_id, _) = create_portal(&emulator, case, "Disabled Portal").await;

        let mut last_description = row.published_description;
        for description in [Some(row.published_description), row.republished_description]
            .into_iter()
            .flatten()
        {
            accepted(
                &emulator,
                case,
                Build::Publish,
                &portal_id,
                Some(json!({ "description": description })),
            )
            .await;
            sleep(past_the_build());
            last_description = description;
        }

        accepted(&emulator, case, Build::Disable, &portal_id, Option::None).await;

        // Removing the publication is the SERVICE's work, so the portal reports it
        // in progress until the build this call started comes due. (That the
        // portal's DOMAIN stops serving from the instant the disable is accepted,
        // rather than when this status settles, is asserted in
        // `portal_publication_revocation`.)
        let building = get_portal(&emulator, case, &portal_id).await;
        assert_eq!(
            publish_status(case, &building),
            PUBLISH_STATUS_DISABLE_IN_PROGRESS,
            "case {case:?}: expected the disable to be in progress, got {building}"
        );

        // The transition is owed by the CLOCK: the same read reports it settled with
        // nothing having been called in between.
        sleep(past_the_build());
        let settled = get_portal(&emulator, case, &portal_id).await;
        assert_eq!(
            publish_status(case, &settled),
            PUBLISH_STATUS_DISABLED,
            "case {case:?}: expected the disable to come due and report \
             {PUBLISH_STATUS_DISABLED}, got {settled}"
        );

        // A disable removes the publication, not the record of the last one there was.
        assert_eq!(
            as_str(&settled, "lastPublishedDescription"),
            last_description,
            "case {case:?}: expected the disabled portal to keep the description of \
             the publication it had, got {settled}"
        );
        assert_instant_now(case, &settled, "lastPublished");
    }
}

/// One preview row: the publication state the portal is in when the preview starts, and
/// the `PublishStatus` that state reports — which the preview may not change.
struct PreviewRow {
    case: &'static str,
    /// The builds to run (and settle) before the preview.
    before: &'static [Build],
    expect_publish_status: &'static str,
}

#[tokio::test]
async fn preview_portal_builds_a_preview_without_changing_what_the_portal_publishes() {
    let emulator = emulator();

    for row in [
        PreviewRow {
            case: "a portal that has never been published",
            before: &[],
            expect_publish_status: PUBLISH_STATUS_DISABLED,
        },
        PreviewRow {
            case: "a published portal",
            before: &[Build::Publish],
            expect_publish_status: PUBLISH_STATUS_PUBLISHED,
        },
        PreviewRow {
            case: "a portal whose publication was taken down",
            before: &[Build::Publish, Build::Disable],
            expect_publish_status: PUBLISH_STATUS_DISABLED,
        },
    ] {
        let case = row.case;
        let (portal_id, default_domain) = create_portal(&emulator, case, "Previewed Portal").await;
        for build in row.before {
            accepted(&emulator, case, *build, &portal_id, Option::None).await;
            sleep(past_the_build());
        }

        // Nothing has been previewed, so the preview the portal reports carries nothing:
        // the member is always there, as the empty structure.
        let before = get_portal(&emulator, case, &portal_id).await;
        assert_eq!(
            before.get("preview"),
            Some(&json!({})),
            "case {case:?}: a portal nothing has previewed reports the empty preview, got \
             {before}"
        );

        accepted(&emulator, case, Build::Preview, &portal_id, Option::None).await;

        // The preview URL names THIS portal, and is served at the address this emulator
        // serves the portal's site at — a `host:port` on this machine, since a client has
        // to be able to fetch it — under the preview's own path.
        let building = get_portal(&emulator, case, &portal_id).await;
        let status = preview_member(case, &building, "previewStatus");
        assert!(
            PREVIEW_STATUS_VALUES.contains(&status.as_str()),
            "case {case:?}: expected a modelled PreviewStatus out of {PREVIEW_STATUS_VALUES:?}, got {building}"
        );
        assert_eq!(
            status, PREVIEW_STATUS_PREVIEW_IN_PROGRESS,
            "case {case:?}: expected the started preview to be in progress, got {building}"
        );
        let expected_url = preview_member(case, &building, "previewUrl");
        let fetchable = expected_url
            .strip_prefix("https://")
            .and_then(|rest| rest.strip_suffix("/preview"))
            .and_then(|authority| authority.parse::<std::net::SocketAddr>().ok())
            .is_some_and(|address| address.ip().is_loopback() && address.port() != 0);
        assert!(
            fetchable,
            "case {case:?}: expected a preview URL a client on this machine can fetch, got \
             {expected_url:?}"
        );
        // The preview URL carries the dialable ADDRESS this emulator serves the site at (the
        // `host:port` asserted just above), which is the SAME address the portal publishes as
        // its default domain: both send a client to one site, so the one address this emulator
        // serves it at is what both name, the URL adding the scheme and the preview's own path
        // that a URL carries. The form of that member is
        // `portal_domain_name_is_not_an_authority`'s subject.
        assert_eq!(
            expected_url,
            format!("https://{default_domain}/preview"),
            "case {case:?}: expected the preview URL on the address the portal publishes its \
             site at, got {expected_url:?} beside {default_domain:?}"
        );
        assert_eq!(
            publish_status(case, &building),
            row.expect_publish_status,
            "case {case:?}: expected a preview to leave the PublishStatus alone, got {building}"
        );

        sleep(past_the_build());
        let settled = get_portal(&emulator, case, &portal_id).await;
        assert_eq!(
            preview_member(case, &settled, "previewStatus"),
            PREVIEW_STATUS_PREVIEW_READY,
            "case {case:?}: expected the preview to come due and report \
             {PREVIEW_STATUS_PREVIEW_READY}, got {settled}"
        );
        assert_eq!(
            preview_member(case, &settled, "previewUrl"),
            expected_url,
            "case {case:?}: expected the preview URL to be stable across reads, got {settled}"
        );
        assert_eq!(
            publish_status(case, &settled),
            row.expect_publish_status,
            "case {case:?}: expected a settled preview to leave the PublishStatus alone, got {settled}"
        );
    }
}

#[tokio::test]
async fn a_portal_refuses_only_a_build_that_needs_what_another_is_already_doing() {
    let emulator = emulator();

    // Every ORDERED pair of the three builds. Whether the second one is refused follows
    // from what the two NEED, not from which calls they are.
    for first in EVERY_BUILD {
        for second in EVERY_BUILD {
            let case = format!("{first:?} in flight, then {second:?}");
            let case = case.as_str();
            let (portal_id, _) = create_portal(&emulator, case, "Serial Portal").await;

            // Every pair starts from a portal with a LIVE publication, because that is
            // what a disable needs to have something to take down
            // (`PORTAL_DISABLE_FROM_STATUSES`); so the only difference between the rows
            // is the pair of builds.
            accepted(&emulator, case, Build::Publish, &portal_id, Option::None).await;
            sleep(past_the_build());

            accepted(&emulator, case, first, &portal_id, Option::None).await;

            if second.is_held_up_by(first) {
                let (status, refused) = start(&emulator, second, &portal_id, Option::None).await;
                assert_eq!(
                    status, 409,
                    "case {case:?}: expected 409 while {} is in flight, got {status} {refused}",
                    first.in_progress()
                );
                assert_eq!(
                    error_code(&refused),
                    "ConflictException",
                    "case {case:?}: expected ConflictException, got {refused}"
                );

                // The refusal wrote nothing: the portal is still running the FIRST build.
                let mid = get_portal(&emulator, case, &portal_id).await;
                assert_eq!(
                    first.reported_status(case, &mid),
                    first.in_progress(),
                    "case {case:?}: expected the refused call to leave the first build \
                     running, got {mid}"
                );

                // The refusal lifts of its own accord once the build the service owed
                // comes due — nothing but the clock has to happen. The one pair it does
                // not lift for is a disable behind a disable: the first one leaves the
                // portal with no publication, and a portal publishing nothing is in no
                // status a publication can be taken down from.
                sleep(past_the_build());
                if first == Build::Disable && second == Build::Disable {
                    let (status, refused) =
                        start(&emulator, second, &portal_id, Option::None).await;
                    assert_eq!(
                        status, 409,
                        "case {case:?}: expected the second disable to stay refused once the \
                         first had come due, got {status} {refused}"
                    );
                    let settled = get_portal(&emulator, case, &portal_id).await;
                    assert_eq!(
                        publish_status(case, &settled),
                        PUBLISH_STATUS_DISABLED,
                        "case {case:?}: expected the refused second disable to leave the \
                         portal as the first one left it, got {settled}"
                    );
                } else {
                    accepted(&emulator, case, second, &portal_id, Option::None).await;
                }
            } else {
                // The two need different things, so they run side by side and the portal
                // reports both of them in flight at once.
                accepted(&emulator, case, second, &portal_id, Option::None).await;
                let both = get_portal(&emulator, case, &portal_id).await;
                for build in [first, second] {
                    assert_eq!(
                        build.reported_status(case, &both),
                        build.in_progress(),
                        "case {case:?}: expected {build:?} to be reported in flight \
                         alongside the other, got {both}"
                    );
                }
            }

            // Whatever ran, the CLOCK settles it: nothing is left in an
            // `..._IN_PROGRESS` status once the span has passed, and the portal's
            // publication is in one of its terminal values.
            sleep(past_the_build());
            let settled = get_portal(&emulator, case, &portal_id).await;
            for build in [first, second] {
                assert_ne!(
                    build.reported_status(case, &settled),
                    build.in_progress(),
                    "case {case:?}: expected {build:?} to come due and leave {}, got {settled}",
                    build.in_progress()
                );
            }
            assert!(
                [PUBLISH_STATUS_PUBLISHED, PUBLISH_STATUS_DISABLED]
                    .contains(&publish_status(case, &settled).as_str()),
                "case {case:?}: expected a terminal PublishStatus once every span has \
                 passed, got {settled}"
            );
        }
    }
}

#[tokio::test]
async fn the_portal_build_calls_refuse_an_id_nothing_minted() {
    let emulator = emulator();
    // Ten lowercase alphanumerics — the form of a minted `PortalId`, so what is refused
    // is the id naming no portal rather than its shape.
    let unminted = "zzzzzzzzzz";

    for build in EVERY_BUILD {
        let case = format!("{build:?} on an id nothing minted");
        let case = case.as_str();
        let (status, body) = start(&emulator, build, unminted, Option::None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for a portal the account does not hold, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }
}

#[tokio::test]
async fn the_portal_build_status_names_are_the_modelled_catalogs() {
    for status in [
        PUBLISH_STATUS_DISABLED,
        PUBLISH_STATUS_DISABLE_IN_PROGRESS,
        PUBLISH_STATUS_PUBLISHED,
        PUBLISH_STATUS_PUBLISH_IN_PROGRESS,
    ] {
        assert!(
            PUBLISH_STATUS_VALUES.contains(&status),
            "case {status:?}: expected a PublishStatus the model declares, out of {PUBLISH_STATUS_VALUES:?}"
        );
    }
    for status in [
        PREVIEW_STATUS_PREVIEW_IN_PROGRESS,
        PREVIEW_STATUS_PREVIEW_READY,
    ] {
        assert!(
            PREVIEW_STATUS_VALUES.contains(&status),
            "case {status:?}: expected a PreviewStatus the model declares, out of {PREVIEW_STATUS_VALUES:?}"
        );
    }
}
