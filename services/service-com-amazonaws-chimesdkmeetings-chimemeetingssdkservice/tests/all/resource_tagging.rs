// SPDX-License-Identifier: CC0-1.0

//! The generic tagging surface of this service, end to end over HTTP.
//!
//! The RULE pinned here: a resource's tags are ARN-keyed state the three generic tagging
//! routes share. `TagResource` MERGES the pairs it carries into what the resource already
//! has (a repeated key is re-valued, an untouched one kept), `UntagResource` removes
//! exactly the keys it names and nothing else, and `ListTagsForResource` reports back the
//! tags those two and the CREATE left — an empty list for a resource carrying none. A
//! tagging call whose ARN names a MEETING this store holds no meeting under — one that
//! never existed, or one `DeleteMeeting` has ended — is refused with the modelled
//! `BadRequestException` reporting the id in it as invalid and writes nothing, a call whose
//! ARN names no resource of the Chime family at all with the modelled
//! `ResourceNotFoundException`, and one that would take a resource past the modelled
//! maximum number of tags per resource with the modelled `TooManyTagsException`.
//!
//! `ListTagsForResource`, `TagResource` and `UntagResource` also answer ONE verdict for one
//! ARN: whether the call may be served at all, and which refusal it earns when it may not,
//! is one decision the three routes share, so none of them can refuse an ARN the other two
//! serve — pinned by `every_tagging_route_answers_one_arn_with_one_verdict`. That verdict
//! reads the resource TYPE the ARN's resource field names to decide whose resource it is, so
//! every spelling of this service's own type is resolved against this store and one absent
//! meeting cannot earn opposite verdicts by being spelled two ways — pinned by
//! `every_spelling_of_this_services_resource_type_resolves_against_this_store`.
//!
//! The tables span what the rule generalises over: how many pairs one call carries (one,
//! several, the modelled maximum, one past it), the form of a key and a value (short, an
//! EMPTY value, at the modelled length bounds), which create minted the resource
//! (`CreateMeeting` and `CreateMeetingWithAttendees`), how many keys an untag names (one,
//! several, one the resource never carried), and the order the calls arrive in.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_chimesdkmeetings_chimemeetingssdkservice::state::{
    INVALID_MEETING_ID_MESSAGE as INVALID_MEETING_ID, MAX_TAGS_PER_RESOURCE,
};
use service_com_amazonaws_chimesdkmeetings_chimemeetingssdkservice::EmulatedService;
use service_common::chime_tags::{ChimeTagDirectory, WithdrawnClass};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names the `chime` signing name
/// this service publishes, as the sibling suites in this binary state it.
const SIGNED_FOR_CHIME: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTCHIME/20260101/us-east-1/chime/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The emulator under test: ONE shared engine with this service mounted on it.
struct Emulator {
    router: MultiServiceRouter,
    /// The same engine the service is mounted on, kept so a test can act as the family
    /// member that OWNS a sibling's resource — the only way an ARN of another member
    /// reaches this service's tagging routes as a withdrawn one.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Record in the family's shared ARN-keyed directory that the member OWNING
    /// `resource_arn` has deleted the resource, with the class and wording that member
    /// serves — exactly what that sibling's own delete writes there.
    fn withdraw(&self, resource_arn: &str, class: WithdrawnClass, message: &str) {
        self.engine
            .state::<ChimeTagDirectory>()
            .lock()
            .withdraw(resource_arn, class, message);
    }

    /// What each of the three generic tagging routes answers for `arn`, with the status
    /// that route's model declares for a SERVED call: one call site, so a table states one
    /// expectation for the three routes instead of three.
    async fn tagging_routes(&self, arn: &str) -> Vec<(&'static str, u16, u16, Value)> {
        let (list_status, list_body) = self.list_raw(arn).await;
        let (tag_status, tag_body) = self.tag(arn, &owned(&[("env", "dev")])).await;
        let (untag_status, untag_body) = self.untag(arn, &keys(&["env"])).await;
        vec![
            ("ListTagsForResource", 200, list_status, list_body),
            ("TagResource", 204, tag_status, tag_body),
            ("UntagResource", 204, untag_status, untag_body),
        ]
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://meetings-chime.us-east-1.amazonaws.com{path}"
            ))
            .header("host", "meetings-chime.us-east-1.amazonaws.com")
            .header("authorization", SIGNED_FOR_CHIME)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, body)
    }

    /// Create a meeting and return its `(MeetingId, MeetingArn)`.
    async fn meeting(
        &self,
        token: &str,
        external_id: &str,
        tags: &[(String, String)],
    ) -> (String, String) {
        let mut request = json!({
            "ClientRequestToken": token,
            "MediaRegion": "us-east-1",
            "ExternalMeetingId": external_id,
        });
        if !tags.is_empty() {
            request["Tags"] = wire_tags(tags);
        }
        let (status, body) = self.call("POST", "/meetings", Some(&request)).await;
        assert_eq!(
            status, 200,
            "CreateMeeting({external_id}) must answer 200, got {status} {body:?}"
        );
        ids_of(&body["Meeting"])
    }

    /// Create a meeting through `CreateMeetingWithAttendees` and return the same pair, so
    /// the rule is pinned over BOTH creates that may carry tags.
    async fn meeting_with_attendees(
        &self,
        token: &str,
        external_id: &str,
        tags: &[(String, String)],
    ) -> (String, String) {
        let mut request = json!({
            "ClientRequestToken": token,
            "MediaRegion": "us-east-1",
            "ExternalMeetingId": external_id,
            "Attendees": [{ "ExternalUserId": "user-1" }],
        });
        if !tags.is_empty() {
            request["Tags"] = wire_tags(tags);
        }
        let (status, body) = self
            .call("POST", "/meetings?operation=create-attendees", Some(&request))
            .await;
        assert_eq!(
            status, 200,
            "CreateMeetingWithAttendees({external_id}) must answer 200, got {status} {body:?}"
        );
        ids_of(&body["Meeting"])
    }

    async fn tag(&self, arn: &str, pairs: &[(String, String)]) -> (u16, Value) {
        self.call(
            "POST",
            "/tags?operation=tag-resource",
            Some(&json!({ "ResourceARN": arn, "Tags": wire_tags(pairs) })),
        )
        .await
    }

    async fn untag(&self, arn: &str, tag_keys: &[String]) -> (u16, Value) {
        self.call(
            "POST",
            "/tags?operation=untag-resource",
            Some(&json!({ "ResourceARN": arn, "TagKeys": tag_keys })),
        )
        .await
    }

    async fn list_raw(&self, arn: &str) -> (u16, Value) {
        self.call("GET", &format!("/tags?arn={}", encode(arn)), None)
            .await
    }

    async fn list(&self, arn: &str) -> (u16, BTreeMap<String, String>) {
        let (status, body) = self.list_raw(arn).await;
        (status, tag_map(&body))
    }

    /// The pairs the family's shared ARN-keyed directory holds under `resource_arn`, read
    /// from the engine this service is mounted on.
    ///
    /// The only way to see what a REFUSED call wrote: every route of an ARN the verdict
    /// refuses answers that refusal, so no response can report the tags stored under it.
    fn stored_tags(&self, resource_arn: &str) -> Vec<(String, String)> {
        self.engine
            .state::<ChimeTagDirectory>()
            .lock()
            .tags_of(resource_arn)
    }
}

/// The `(MeetingId, MeetingArn)` of a created meeting.
fn ids_of(meeting: &Value) -> (String, String) {
    let id = meeting["MeetingId"]
        .as_str()
        .unwrap_or_else(|| panic!("no MeetingId in {meeting:?}"))
        .to_string();
    let arn = meeting["MeetingArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no MeetingArn in {meeting:?}"))
        .to_string();
    (id, arn)
}

/// Percent-encode a value for a query value, the way an AWS SDK encodes one.
///
/// `pub(crate)` so the sibling suites of this test binary share this ONE copy rather than
/// each writing an encoder of their own, which would let two suites assert against
/// different encodings of the same ARN.
pub(crate) fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn wire_tags(pairs: &[(String, String)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

fn tag_map(body: &Value) -> BTreeMap<String, String> {
    body["Tags"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|tag| {
                    Some((
                        tag["Key"].as_str()?.to_string(),
                        tag["Value"].as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn owned(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

fn keys(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

fn as_map(pairs: &[(String, String)]) -> BTreeMap<String, String> {
    pairs.iter().cloned().collect()
}

/// One row of the round-trip table: the pairs each successive tagging call applies, the
/// keys untagged afterwards, and the tags the resource is left with.
struct TagCase {
    case: &'static str,
    calls: Vec<Vec<(String, String)>>,
    removed: Vec<String>,
    left: Vec<(String, String)>,
}

/// RULE: tagging merges, untagging removes only the keys it names, and the listing
/// reports exactly the tags the writes left.
#[tokio::test]
async fn tags_round_trip_through_the_shared_tagging_routes() {
    let emulator = Emulator::new();

    // The pairs below stay inside the model's own bounds: `TagKey` is len 1..128 and
    // `TagValue` len 0..256, both over the pattern this model declares
    // (`smithy_query(query="shape",shape="Tag")`), so an EMPTY value is a value a row may
    // carry.
    let long_key = "k".repeat(128);
    let long_value = "v".repeat(256);
    let at_bounds = owned(&[
        (long_key.as_str(), long_value.as_str()),
        ("a.b:c/d_e+f", ""),
    ]);

    let rows: Vec<TagCase> = vec![
        TagCase {
            case: "one pair, kept",
            calls: vec![owned(&[("env", "dev")])],
            removed: Vec::new(),
            left: owned(&[("env", "dev")]),
        },
        TagCase {
            case: "several pairs in one call, one of them removed again",
            calls: vec![owned(&[("env", "dev"), ("team", "core"), ("tier", "gold")])],
            removed: keys(&["team"]),
            left: owned(&[("env", "dev"), ("tier", "gold")]),
        },
        TagCase {
            case: "two calls merging, the repeated key re-valued",
            calls: vec![
                owned(&[("env", "dev"), ("team", "core")]),
                owned(&[("env", "prod"), ("tier", "gold")]),
            ],
            removed: Vec::new(),
            left: owned(&[("env", "prod"), ("team", "core"), ("tier", "gold")]),
        },
        TagCase {
            case: "a key the resource never carried is removed as a no-op",
            calls: vec![owned(&[("env", "dev")])],
            removed: keys(&["absent", "neither"]),
            left: owned(&[("env", "dev")]),
        },
        TagCase {
            case: "every key removed, leaving no tags at all",
            calls: vec![owned(&[("env", "dev"), ("team", "core")])],
            removed: keys(&["env", "team"]),
            left: Vec::new(),
        },
        TagCase {
            case: "keys and values at the modelled length bounds, and an empty value",
            calls: vec![at_bounds.clone()],
            removed: Vec::new(),
            left: at_bounds,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let TagCase {
            case,
            calls,
            removed,
            left,
        } = row;
        // Both creates that may carry tags, so the rule is pinned over the way the
        // resource was minted as well as over the tags.
        let (_, plain) = emulator
            .meeting(&format!("tok-tag-{index}"), &format!("ext-tag-{index}"), &[])
            .await;
        let (_, with_attendees) = emulator
            .meeting_with_attendees(
                &format!("tok-tag-{index}-a"),
                &format!("ext-tag-{index}-a"),
                &[],
            )
            .await;

        for arn in [plain.as_str(), with_attendees.as_str()] {
            let (status, tags) = emulator.list(arn).await;
            assert_eq!(
                (status, tags.len()),
                (200, 0),
                "case {case:?} ({arn}): an untagged resource reports no tags, got {tags:?}"
            );

            for pairs in &calls {
                let (status, body) = emulator.tag(arn, pairs).await;
                assert_eq!(
                    status, 204,
                    "case {case:?} ({arn}): expected TagResource to answer 204, got \
                     {status} {body:?}"
                );
            }
            if !removed.is_empty() {
                let (status, body) = emulator.untag(arn, &removed).await;
                assert_eq!(
                    status, 204,
                    "case {case:?} ({arn}): expected UntagResource to answer 204, got \
                     {status} {body:?}"
                );
            }

            let (status, tags) = emulator.list(arn).await;
            assert_eq!(
                (status, &tags),
                (200, &as_map(&left)),
                "case {case:?} ({arn}): expected exactly the tags the writes left"
            );

            // A read does not change what the next reader sees.
            let (status, again) = emulator.list(arn).await;
            assert_eq!(
                (status, &again),
                (200, &tags),
                "case {case:?} ({arn}): a second listing differed from the first"
            );
        }
    }
}

/// One row of the create-time table: the tags the create carries, what the later calls do
/// to them, and the tags the resource is left with.
struct CreateTagCase {
    case: &'static str,
    created_with: Vec<(String, String)>,
    then_tagged: Vec<(String, String)>,
    untagged: Vec<String>,
    left: Vec<(String, String)>,
}

/// RULE: tags given at CREATE time are the same state the tagging routes read and write,
/// so a created resource lists them, the tagging routes change them, and deleting the
/// resource takes them with it.
#[tokio::test]
async fn tags_supplied_at_create_time_are_listed_and_can_be_changed() {
    let emulator = Emulator::new();

    let rows: Vec<CreateTagCase> = vec![
        CreateTagCase {
            case: "one create-time tag, left alone",
            created_with: owned(&[("env", "dev")]),
            then_tagged: Vec::new(),
            untagged: Vec::new(),
            left: owned(&[("env", "dev")]),
        },
        CreateTagCase {
            case: "three create-time tags, one re-valued and one dropped",
            created_with: owned(&[("env", "dev"), ("team", "core"), ("tier", "gold")]),
            then_tagged: owned(&[("env", "prod")]),
            untagged: keys(&["tier"]),
            left: owned(&[("env", "prod"), ("team", "core")]),
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let CreateTagCase {
            case,
            created_with,
            then_tagged,
            untagged,
            left,
        } = row;
        let (meeting_id, arn) = emulator
            .meeting(
                &format!("tok-create-tag-{index}"),
                &format!("ext-create-tag-{index}"),
                &created_with,
            )
            .await;
        let (_, with_attendees) = emulator
            .meeting_with_attendees(
                &format!("tok-create-tag-{index}-a"),
                &format!("ext-create-tag-{index}-a"),
                &created_with,
            )
            .await;

        for tagged in [arn.as_str(), with_attendees.as_str()] {
            let (status, listed) = emulator.list(tagged).await;
            assert_eq!(
                (status, &listed),
                (200, &as_map(&created_with)),
                "case {case:?} ({tagged}): expected the create-time tags to be listed"
            );

            if !then_tagged.is_empty() {
                emulator.tag(tagged, &then_tagged).await;
            }
            if !untagged.is_empty() {
                emulator.untag(tagged, &untagged).await;
            }

            let (status, listed) = emulator.list(tagged).await;
            assert_eq!(
                (status, &listed),
                (200, &as_map(&left)),
                "case {case:?} ({tagged}): expected the create-time tags as the later calls \
                 left them"
            );
        }

        // The resource's tags go with the resource: once the meeting is deleted, its ARN
        // carries a `MeetingId` this service holds no meeting under, so every tagging call
        // naming it is refused as invalid.
        let (status, body) = emulator
            .call("DELETE", &format!("/meetings/{meeting_id}"), None)
            .await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected DeleteMeeting to answer 204, got {status} {body:?}"
        );
        let refusals = [
            ("ListTagsForResource", emulator.list_raw(&arn).await),
            (
                "TagResource",
                emulator.tag(&arn, &owned(&[("after", "delete")])).await,
            ),
            ("UntagResource", emulator.untag(&arn, &keys(&["env"])).await),
        ];
        for (operation, (status, body)) in refusals {
            assert_eq!(
                (status, body["Code"].as_str(), body["Message"].as_str()),
                (400, Some("BadRequest"), Some(INVALID_MEETING_ID)),
                "case {case:?} ({operation}): expected a deleted meeting's ARN to be refused \
                 as an invalid meeting id, got {status} {body:?}"
            );
        }
    }
}

/// RULE: a tagging call whose ARN names a meeting this store holds no meeting under is
/// refused with the modelled 400 reporting the id as invalid and writes nothing; one whose
/// ARN names no resource of this family at all is refused with the modelled 404; and a
/// call past the modelled maximum number of tags per resource is refused with the modelled
/// too-many-tags error, leaving the resource's tags as they were.
#[tokio::test]
async fn a_tagging_call_this_service_cannot_serve_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let (meeting_id, arn) = emulator.meeting("tok-refuse", "ext-refuse", &[]).await;
    let (ended_id, ended_arn) = emulator.meeting("tok-ended", "ext-ended", &[]).await;
    let (status, body) = emulator
        .call("DELETE", &format!("/meetings/{ended_id}"), None)
        .await;
    assert_eq!(
        status, 204,
        "DeleteMeeting on the meeting that is ended for the table: got {status} {body:?}"
    );

    // Rows: a meeting ARN this store holds no meeting for, the live meeting's id in
    // another Region (which names no meeting here, because a meeting's ARN carries the
    // Region of the endpoint that created it), and a meeting that EXISTED and has ended.
    let invalid: Vec<(&str, String)> = vec![
        ("a meeting that never existed", format!("{arn}x")),
        (
            "the live meeting's id in another Region",
            arn.replace("us-east-1", "eu-west-1"),
        ),
        ("a meeting that has been deleted", ended_arn),
    ];

    for (case, invalid_arn) in invalid {
        let refusals = [
            (
                "TagResource",
                emulator.tag(&invalid_arn, &owned(&[("k", "v")])).await,
            ),
            (
                "UntagResource",
                emulator.untag(&invalid_arn, &keys(&["k"])).await,
            ),
            ("ListTagsForResource", emulator.list_raw(&invalid_arn).await),
        ];
        for (operation, (status, payload)) in refusals {
            assert_eq!(
                (
                    status,
                    payload["Code"].as_str(),
                    payload["Message"].as_str()
                ),
                (400, Some("BadRequest"), Some(INVALID_MEETING_ID)),
                "case {case:?} ({operation}): expected 400 BadRequest {INVALID_MEETING_ID:?} \
                 for {invalid_arn}, got {status} {payload:?}"
            );
        }
    }

    // An ARN that names no resource of this family at all is a different mistake: there is
    // no meeting id in it to report as invalid.
    let foreign = format!("arn:aws:sqs:us-east-1:000000000000:queue/{meeting_id}");
    let refusals = [
        ("TagResource", emulator.tag(&foreign, &owned(&[("k", "v")])).await),
        ("UntagResource", emulator.untag(&foreign, &keys(&["k"])).await),
        ("ListTagsForResource", emulator.list_raw(&foreign).await),
    ];
    for (operation, (status, payload)) in refusals {
        assert_eq!(
            (status, payload["Code"].as_str()),
            (404, Some("NotFound")),
            "case another vendor's resource ARN ({operation}): expected 404 NotFound for \
             {foreign}, got {status} {payload:?}"
        );
        assert_eq!(
            payload["ResourceName"].as_str(),
            Some(foreign.as_str()),
            "case another vendor's resource ARN ({operation}): expected the refusal to name \
             the resource it could not find, got {payload:?}"
        );
    }

    // The tag count is bounded by the model's own `TagList` maximum: a call that would
    // take the resource past it is refused and writes nothing.
    let at_maximum: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("k{index:02}"), "v".to_string()))
        .collect();
    let (status, body) = emulator.tag(&arn, &at_maximum).await;
    assert_eq!(
        status, 204,
        "case the modelled maximum of tags: expected it accepted, got {status} {body:?}"
    );

    let (status, body) = emulator.tag(&arn, &owned(&[("one-too-many", "v")])).await;
    assert_eq!(
        (status, body["Code"].as_str()),
        (400, Some("TooManyTags")),
        "case one tag past the maximum: expected the modelled too-many-tags refusal, got \
         {status} {body:?}"
    );
    let (status, listed) = emulator.list(&arn).await;
    assert_eq!(
        (status, listed.len()),
        (200, at_maximum.len()),
        "case one tag past the maximum: expected the refused call to write nothing"
    );

    // Re-valuing a key the resource already carries adds none, so it is accepted even at
    // the maximum.
    let (status, body) = emulator.tag(&arn, &owned(&[("k00", "re-valued")])).await;
    assert_eq!(
        status, 204,
        "case re-valuing a key at the maximum: expected it accepted, got {status} {body:?}"
    );
    let (status, listed) = emulator.list(&arn).await;
    assert_eq!(
        (status, listed.get("k00").map(String::as_str)),
        (200, Some("re-valued")),
        "case re-valuing a key at the maximum: expected the new value, got {listed:?}"
    );
}

/// What the three tagging routes must answer for one ARN.
enum Verdict {
    /// The call may be addressed to the ARN, so each route answers the success status its
    /// own model declares.
    Served,
    /// The call may not, so each route answers this status with this `Code` and `Message` —
    /// the same refusal, worded once, whichever route read the ARN.
    Refused {
        status: u16,
        code: &'static str,
        message: String,
    },
}

/// One row of the verdict table: the ARN a tagging call names and the one answer every
/// route owes it.
struct VerdictCase {
    case: &'static str,
    arn: String,
    verdict: Verdict,
}

/// RULE: whether a generic tagging call may address the `ResourceARN` it named — and, when
/// it may not, the class and the wording of the refusal — is ONE verdict resolved from this
/// service's own store and the family's shared ARN-keyed directory, so `TagResource`,
/// `UntagResource` and `ListTagsForResource` answer the same ARN the same way and none of
/// them refuses one the other two serve.
///
/// The rows span every answer that verdict has: served, an ARN of this service naming a
/// meeting the store does not hold, an ARN that names no resource of this family at all, and
/// an ARN whose OWNING family member has withdrawn the resource under each class that owner
/// may choose.
#[tokio::test]
async fn every_tagging_route_answers_one_arn_with_one_verdict() {
    let emulator = Emulator::new();
    let (_, live) = emulator.meeting("tok-verdict", "ext-verdict", &[]).await;
    let (ended_id, ended) = emulator
        .meeting("tok-verdict-ended", "ext-verdict-ended", &[])
        .await;
    let (status, body) = emulator
        .call("DELETE", &format!("/meetings/{ended_id}"), None)
        .await;
    assert_eq!(
        status, 204,
        "DeleteMeeting on the meeting the table ends: got {status} {body:?}"
    );

    // ARNs of SIBLING members of the family: resource types this service does not mint, so
    // its own store cannot resolve them and the shared directory is what says whether the
    // member that owns them still holds them.
    let sibling_live = "arn:aws:chime:us-east-1:000000000000:app-instance/ai-live".to_string();
    let sibling_invalid = "arn:aws:chime:us-east-1:000000000000:app-instance/ai-gone".to_string();
    let sibling_forbidden =
        "arn:aws:chime:us-east-1:000000000000:media-pipeline/mp-gone".to_string();
    // The wording is the OWNER's, so each withdrawal carries its own sentence and the
    // assertions below show the serving route passed that owner's words through rather than
    // picking one of its own.
    let invalid_request_wording = "Invalid app instance id";
    let forbidden_wording = "The media pipeline is no longer available";
    emulator.withdraw(
        &sibling_invalid,
        WithdrawnClass::InvalidRequest,
        invalid_request_wording,
    );
    emulator.withdraw(
        &sibling_forbidden,
        WithdrawnClass::Forbidden,
        forbidden_wording,
    );

    let rows = vec![
        VerdictCase {
            case: "a live meeting of this service",
            arn: live.clone(),
            verdict: Verdict::Served,
        },
        VerdictCase {
            case: "a live resource of a sibling member, which this store cannot check",
            arn: sibling_live,
            verdict: Verdict::Served,
        },
        VerdictCase {
            case: "a meeting of this service that was never created",
            arn: format!("{live}x"),
            verdict: Verdict::Refused {
                status: 400,
                code: "BadRequest",
                message: INVALID_MEETING_ID.to_string(),
            },
        },
        VerdictCase {
            case: "a meeting of this service that has been deleted",
            arn: ended,
            verdict: Verdict::Refused {
                status: 400,
                code: "BadRequest",
                message: INVALID_MEETING_ID.to_string(),
            },
        },
        VerdictCase {
            case: "a value that begins like an ARN but names no resource",
            arn: "arn:aws:chime:us-east-1:000000000000".to_string(),
            verdict: Verdict::Refused {
                status: 404,
                code: "NotFound",
                message: "resource arn:aws:chime:us-east-1:000000000000 does not exist"
                    .to_string(),
            },
        },
        VerdictCase {
            case: "another vendor's resource ARN",
            arn: "arn:aws:sqs:us-east-1:000000000000:queue/q1".to_string(),
            verdict: Verdict::Refused {
                status: 404,
                code: "NotFound",
                message: "resource arn:aws:sqs:us-east-1:000000000000:queue/q1 does not exist"
                    .to_string(),
            },
        },
        VerdictCase {
            case: "a sibling's resource its owner withdrew as an invalid request",
            arn: sibling_invalid,
            verdict: Verdict::Refused {
                status: 400,
                code: "BadRequest",
                message: invalid_request_wording.to_string(),
            },
        },
        VerdictCase {
            case: "a sibling's resource its owner withdrew as forbidden",
            arn: sibling_forbidden,
            verdict: Verdict::Refused {
                status: 403,
                code: "Forbidden",
                message: forbidden_wording.to_string(),
            },
        },
    ];

    for row in rows {
        let VerdictCase { case, arn, verdict } = row;
        for (route, served_status, status, body) in emulator.tagging_routes(&arn).await {
            match &verdict {
                Verdict::Served => assert_eq!(
                    status, served_status,
                    "case {case:?} ({route} on {arn}): expected the call served with \
                     {served_status}, got {status} {body:?}"
                ),
                Verdict::Refused {
                    status: expected_status,
                    code,
                    message,
                } => assert_eq!(
                    (status, body["Code"].as_str(), body["Message"].as_str()),
                    (*expected_status, Some(*code), Some(message.as_str())),
                    "case {case:?} ({route} on {arn}): expected every tagging route to answer \
                     {expected_status} {code} {message:?}, got {status} {body:?}"
                ),
            }
        }
    }
}

/// RULE: which resource TYPE a tagging ARN's resource field names is what says whose
/// resource it is, so an ARN of this family carrying the resource type THIS service mints
/// is resolved against THIS store in every spelling of that field — `<type>/<id>`,
/// `<type>:<id>` and the type written alone, which carries no id — and a meeting the store
/// does not hold is refused as an invalid meeting id whichever way its ARN was spelled,
/// writing nothing. An ARN carrying any OTHER resource type is a sibling member's and is
/// served, in those same spellings.
///
/// The rows differ in the spelling of the resource field: the separator (`/`, `:`, none),
/// whether an id follows it, how many segments it has, and whether the type is this
/// service's own — the dimensions the rule generalises over, each with the one verdict the
/// ARN earns.
#[tokio::test]
async fn every_spelling_of_this_services_resource_type_resolves_against_this_store() {
    let emulator = Emulator::new();
    let (live_id, live) = emulator.meeting("tok-spelling", "ext-spelling", &[]).await;

    // `arn:aws:chime:<region>:<account>:meeting` — the live meeting's own ARN up to and
    // including this service's resource type, so every row below is spelled from the ARN
    // this emulator actually mints rather than from a literal.
    let bare_type = live
        .strip_suffix(&format!("/{live_id}"))
        .unwrap_or_else(|| panic!("the minted ARN {live} must end in /{live_id}"))
        .to_string();
    let invalid_meeting_id = || Verdict::Refused {
        status: 400,
        code: "BadRequest",
        message: INVALID_MEETING_ID.to_string(),
    };
    let unresolved = |arn: &str| Verdict::Refused {
        status: 404,
        code: "NotFound",
        message: format!("resource {arn} does not exist"),
    };

    let rows = vec![
        VerdictCase {
            case: "the live meeting's own ARN, as the create minted it",
            arn: live.clone(),
            verdict: Verdict::Served,
        },
        VerdictCase {
            case: "this service's resource type written alone, with no id at all",
            arn: bare_type.clone(),
            verdict: invalid_meeting_id(),
        },
        VerdictCase {
            case: "this service's resource type with a slash and an empty id",
            arn: format!("{bare_type}/"),
            verdict: invalid_meeting_id(),
        },
        VerdictCase {
            case: "this service's resource type with a colon and an empty id",
            arn: format!("{bare_type}:"),
            verdict: invalid_meeting_id(),
        },
        VerdictCase {
            case: "an id no meeting is held under, under the slash separator",
            arn: format!("{bare_type}/{live_id}x"),
            verdict: invalid_meeting_id(),
        },
        VerdictCase {
            case: "an id no meeting is held under, under the colon separator",
            arn: format!("{bare_type}:{live_id}x"),
            verdict: invalid_meeting_id(),
        },
        VerdictCase {
            case: "the live meeting's id under the separator its ARN does not carry",
            arn: format!("{bare_type}:{live_id}"),
            verdict: invalid_meeting_id(),
        },
        VerdictCase {
            case: "a further segment under the live meeting's id",
            arn: format!("{live}/sub"),
            verdict: invalid_meeting_id(),
        },
        VerdictCase {
            case: "a resource type that merely begins with this service's, with an id",
            arn: format!("{bare_type}s/{live_id}"),
            verdict: Verdict::Served,
        },
        VerdictCase {
            case: "a sibling member's resource type, with an id",
            arn: "arn:aws:chime:us-east-1:000000000000:app-instance/ai-spelling".to_string(),
            verdict: Verdict::Served,
        },
        VerdictCase {
            case: "a sibling member's resource type written alone",
            arn: "arn:aws:chime:us-east-1:000000000000:app-instance".to_string(),
            verdict: Verdict::Served,
        },
        VerdictCase {
            case: "another vendor's ARN carrying this service's resource type alone",
            arn: "arn:aws:sqs:us-east-1:000000000000:meeting".to_string(),
            verdict: unresolved("arn:aws:sqs:us-east-1:000000000000:meeting"),
        },
    ];

    for row in rows {
        let VerdictCase { case, arn, verdict } = row;
        for (route, served_status, status, body) in emulator.tagging_routes(&arn).await {
            match &verdict {
                Verdict::Served => assert_eq!(
                    status, served_status,
                    "case {case:?} ({route} on {arn}): expected the call served with \
                     {served_status}, got {status} {body:?}"
                ),
                Verdict::Refused {
                    status: expected_status,
                    code,
                    message,
                } => assert_eq!(
                    (status, body["Code"].as_str(), body["Message"].as_str()),
                    (*expected_status, Some(*code), Some(message.as_str())),
                    "case {case:?} ({route} on {arn}): expected every tagging route to answer \
                     {expected_status} {code} {message:?}, got {status} {body:?}"
                ),
            }
        }

        if let Verdict::Refused {
            status: expected_status,
            ..
        } = verdict
        {
            // A refused call writes nothing: without this, tags would pile up in the
            // family's shared directory under an ARN no `DeleteMeeting` ever withdraws,
            // and no route of that ARN could report them.
            let (status, body) = emulator.tag(&arn, &owned(&[("residue", "none")])).await;
            let stored = emulator.stored_tags(&arn);
            assert_eq!(
                status, expected_status,
                "case {case:?} (TagResource again on {arn}): expected {expected_status}, got \
                 {status} {body:?}"
            );
            assert!(
                stored.is_empty(),
                "case {case:?}: expected the refused TagResource to write nothing under \
                 {arn}, got {stored:?} stored"
            );
        }
    }
}
