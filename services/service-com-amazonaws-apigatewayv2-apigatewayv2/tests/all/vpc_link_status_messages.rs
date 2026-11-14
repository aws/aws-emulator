// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the `VpcLinkStatusMessage` a VPC link reports.
//!
//! Every test drives the real `restJson1` request path — `MultiServiceRouter::dispatch`
//! over a signed `http::Request` — so what is asserted is the wire behaviour a client
//! sees, not a handler called directly.
//!
//! The rule these cases assert:
//!
//! * **A VPC link's `VpcLinkStatusMessage` is one fixed sentence for the `VpcLinkStatus`
//!   the link reports at the same instant** — the same text for every link in that
//!   status, a different text for a different status, interpolating nothing from the
//!   request or from the record — and every projection of the link (`CreateVpcLink`,
//!   `GetVpcLink`, `GetVpcLinks`, `UpdateVpcLink`) reports the same pair, so a readiness
//!   waiter terminates whichever reader it polls. For the settled status the sentence
//!   itself is pinned, to the one the service is known to serve; the statuses no response
//!   of the service has been observed in are held to the invariants above rather than to
//!   a wording this suite would be inventing.
//!
//! The rows vary what the rule generalises over: the number of subnets and which of the
//! two published EC2 id forms each uses, how many security groups the link carries,
//! whether it carries tags, and the length of its name — none of which the sentence may
//! depend on. The recorded case that filed the issue (a settled link listed by
//! `GetVpcLinks`) is one row read through one of the four readers.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator, error_code, read_until, read_until_gone};

/// The collection URI `CreateVpcLink` and `GetVpcLinks` are bound to; one link hangs off
/// it as `{VPC_LINKS}/{id}`.
const VPC_LINKS: &str = "/v2/vpclinks";

/// The sentence a VPC link that has finished provisioning reports.
///
/// Spelled out here rather than taken from the crate's own constant: a test that asserts a
/// handler's constant back at it cannot catch the wording drifting away from the
/// service's. It is the one sentence the service is known to serve, so the settled status
/// is pinned to it; the other statuses are held to the rule's invariants instead (see
/// [`assert_pair`] and [`assert_one_distinct_sentence_per_status`]), because this suite
/// has no wording of the service's to pin them to.
const AVAILABLE_SENTENCE: &str = "VPC link is ready to route traffic";

/// The `VpcLinkStatus` a link carries while it is being provisioned.
const PENDING: &str = "PENDING";

/// The `VpcLinkStatus` a link carries once it has been provisioned.
const AVAILABLE: &str = "AVAILABLE";

/// The `VpcLinkStatus` a link carries while its deprovisioning is outstanding.
const DELETING: &str = "DELETING";

/// The sentence each `VpcLinkStatus` was seen reporting, as the readings of one case
/// observed them: the rule says there is exactly one per status, so the first reading of a
/// status is what every later reading of it is held to.
type SentencePerStatus = BTreeMap<String, String>;

/// Assert what the rule says of the status/message pair `projection` carries, record the
/// sentence that status reports in `seen`, and answer the status it reported.
///
/// The settled status is held to [`AVAILABLE_SENTENCE`]; every status is held to carrying
/// a summary at all, to carrying the SAME one every other reading of that status carried,
/// and to interpolating none of `carries_nothing_of`.
fn assert_pair(
    case: &str,
    reader: &str,
    projection: &Value,
    carries_nothing_of: &[String],
    seen: &mut SentencePerStatus,
) -> String {
    let status = as_str(projection, "vpcLinkStatus");
    let message = as_str(projection, "vpcLinkStatusMessage");
    assert!(
        !message.is_empty(),
        "case {case:?} ({reader}): expected a VpcLinkStatusMessage summarising \
         VpcLinkStatus {status:?}, got {projection}"
    );
    if status == AVAILABLE {
        assert_eq!(
            message, AVAILABLE_SENTENCE,
            "case {case:?} ({reader}): expected the settled link to report \
             {AVAILABLE_SENTENCE:?}, got {message:?}"
        );
    }
    for value in carries_nothing_of {
        assert!(
            !message.contains(value.as_str()),
            "case {case:?} ({reader}): expected the sentence for {status:?} to carry \
             nothing of the request or the record, but {message:?} carries {value:?}"
        );
    }
    match seen.get(&status) {
        Some(first) => assert_eq!(
            &message, first,
            "case {case:?} ({reader}): expected every link in {status:?} to report the one \
             sentence {first:?}, got {message:?}"
        ),
        None => {
            seen.insert(status.clone(), message);
        }
    }
    status
}

/// Assert the sentences observed tell the statuses APART: a status is summarised by a
/// sentence of its own, so a client reading the message can tell a settled link from one
/// still coming up or going away.
fn assert_one_distinct_sentence_per_status(seen: &SentencePerStatus) {
    for (status, message) in seen {
        for (other, other_message) in seen {
            if other == status {
                continue;
            }
            assert_ne!(
                message, other_message,
                "expected {status:?} and {other:?} to be summarised by sentences of their \
                 own, got {message:?} for both"
            );
        }
    }
}

/// One VPC link the rule is asserted over: the create request, and the values of that
/// request no status sentence may carry.
struct Row {
    case: &'static str,
    request: Value,
    /// Every request value the sentence must be free of — the rule is that it
    /// interpolates nothing from the request.
    carries_nothing_of: Vec<String>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "only the required members: one subnet of the 8-character id form",
            request: json!({ "name": "status-minimal", "subnetIds": ["subnet-0a1b2c3d"] }),
            carries_nothing_of: vec!["status-minimal".to_string(), "subnet-0a1b2c3d".to_string()],
        },
        Row {
            case: "one subnet of the 17-character id form and one security group",
            request: json!({
                "name": "status-long-form-ids",
                "subnetIds": ["subnet-0123456789abcdef0"],
                "securityGroupIds": ["sg-0123456789abcdef0"]
            }),
            carries_nothing_of: vec![
                "status-long-form-ids".to_string(),
                "subnet-0123456789abcdef0".to_string(),
                "sg-0123456789abcdef0".to_string(),
            ],
        },
        Row {
            case: "three subnets mixing both id forms, two security groups, and tags",
            request: json!({
                "name": "status-multi-az",
                "subnetIds": [
                    "subnet-0a1b2c3d",
                    "subnet-00112233445566778",
                    "subnet-ffffffffffffffff0"
                ],
                "securityGroupIds": ["sg-0a1b2c3d", "sg-00112233445566778"],
                "tags": { "team": "platform", "env": "test" }
            }),
            carries_nothing_of: vec![
                "status-multi-az".to_string(),
                "subnet-00112233445566778".to_string(),
                "sg-00112233445566778".to_string(),
                "platform".to_string(),
            ],
        },
        Row {
            // `Name` is a `StringWithLengthBetween1And128`, so this is a link named at the
            // modelled upper bound: the longest name a link can carry changes nothing
            // about its status sentence either.
            case: "a name at the modelled 128-character bound",
            request: json!({
                "name": "n".repeat(128),
                "subnetIds": ["subnet-0a1b2c3d"]
            }),
            carries_nothing_of: vec!["n".repeat(128)],
        },
    ]
}

/// The item `GetVpcLinks` reports for `vpc_link_id`.
async fn listed_item(emulator: &MultiServiceRouter, case: &str, vpc_link_id: &str) -> Value {
    let (status, body) = call(emulator, "GET", VPC_LINKS, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetVpcLinks, got {status} {body}"
    );
    body.get("items")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| as_str(item, "vpcLinkId") == vpc_link_id)
                .cloned()
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected GetVpcLinks to list {vpc_link_id}, got {body}")
        })
}

/// RULE: the status message is the published sentence for the status reported with it,
/// the same from every reader of the link.
#[tokio::test]
async fn every_reader_reports_the_published_sentence_for_the_status_it_reports() {
    let emulator = emulator();

    // Every reading of every row goes in here, so the sentences are held to being one per
    // status ACROSS the rows and the readers, not only within a row.
    let mut seen = SentencePerStatus::new();
    for row in rows() {
        let case = row.case;
        let (status, created) = call(&emulator, "POST", VPC_LINKS, Some(row.request.clone())).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateVpcLink, got {status} {created}"
        );
        let vpc_link_id = as_str(&created, "vpcLinkId");
        let uri = format!("{VPC_LINKS}/{vpc_link_id}");
        // Nothing of the request, and nothing of the record, may be interpolated into a
        // sentence — the minted id included.
        let mut forbidden = row.carries_nothing_of.clone();
        forbidden.push(vpc_link_id.clone());

        // A link is still being provisioned when the create call answers, so the create's
        // own projection carries the sentence for that status.
        let created_status = assert_pair(case, "CreateVpcLink", &created, &forbidden, &mut seen);
        assert_eq!(
            created_status, PENDING,
            "case {case:?}: expected CreateVpcLink to report VpcLinkStatus {PENDING}, got \
             {created}"
        );

        // The provisioning is a transition the clock owes: the read is repeated until the
        // link reports it has settled, never timed.
        let read = read_until(&emulator, case, &uri, |body| {
            as_str(body, "vpcLinkStatus") == AVAILABLE
        })
        .await;
        assert_pair(case, "GetVpcLink", &read, &forbidden, &mut seen);

        // The collection reader and the updater project the settled link the same way.
        let item = listed_item(&emulator, case, &vpc_link_id).await;
        assert_pair(case, "GetVpcLinks", &item, &forbidden, &mut seen);

        let renamed = format!("{}-renamed", as_str(&read, "name"));
        let (status, patched) = call(
            &emulator,
            "PATCH",
            &uri,
            Some(json!({ "name": renamed.chars().take(128).collect::<String>() })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UpdateVpcLink, got {status} {patched}"
        );
        assert_pair(case, "UpdateVpcLink", &patched, &forbidden, &mut seen);
    }

    // Both statuses a created link passes through were observed, and they are summarised
    // differently, so the message moved with the status rather than standing still.
    for status in [PENDING, AVAILABLE] {
        assert!(
            seen.contains_key(status),
            "expected the cases to observe {status:?}, got {seen:?}"
        );
    }
    assert_one_distinct_sentence_per_status(&seen);
}

/// RULE: the same pairing holds while the deprovisioning a delete owes is outstanding —
/// the status moves and the published sentence moves with it.
#[tokio::test]
async fn a_link_being_deprovisioned_reports_the_published_sentence_for_that_status() {
    let emulator = emulator();

    let mut seen = SentencePerStatus::new();
    for row in rows() {
        let case = row.case;
        let (status, created) = call(&emulator, "POST", VPC_LINKS, Some(row.request.clone())).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateVpcLink, got {status} {created}"
        );
        let vpc_link_id = as_str(&created, "vpcLinkId");
        let uri = format!("{VPC_LINKS}/{vpc_link_id}");
        let mut forbidden = row.carries_nothing_of.clone();
        forbidden.push(vpc_link_id.clone());
        let settled = read_until(&emulator, case, &uri, |body| {
            as_str(body, "vpcLinkStatus") == AVAILABLE
        })
        .await;
        assert_pair(case, "GetVpcLink", &settled, &forbidden, &mut seen);

        let (status, accepted) = call(&emulator, "DELETE", &uri, None).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected 202 from DeleteVpcLink, got {status} {accepted}"
        );

        // Every reading the account still serves is held to the rule — including the
        // `DELETING` the delete moved the link to — and the removal the clock owes ends the
        // readings, which is the condition this waits for rather than a time. Which
        // statuses a run happens to catch is not asserted: the deprovisioning may already
        // have fallen due by the first reading, so a case that required a particular
        // reading would turn on how fast the host runs.
        let read = read_until_gone(&emulator, case, &uri, |body| {
            assert_pair(case, "DeleteVpcLink then GetVpcLink", body, &forbidden, &mut seen);
        })
        .await;
        assert_eq!(
            error_code(&read),
            "NotFoundException",
            "case {case:?}: expected the deprovisioned link to be a NotFoundException, got \
             {read}"
        );
    }

    // Whatever the readings caught, a status a link was reported in is summarised by a
    // sentence of its own: a caller cannot read the settled sentence off a link that is
    // going away.
    assert!(
        seen.contains_key(AVAILABLE),
        "expected the cases to observe {AVAILABLE:?} before the delete, got {seen:?}"
    );
    assert_one_distinct_sentence_per_status(&seen);
    if let Some(deleting) = seen.get(DELETING) {
        assert_ne!(
            deleting.as_str(),
            AVAILABLE_SENTENCE,
            "expected a link being deprovisioned not to report the settled sentence"
        );
    }
}
