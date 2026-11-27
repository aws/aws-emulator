// SPDX-License-Identifier: CC0-1.0

//! The two calls that END a participant's connection to a stage, end to end:
//! `DisconnectParticipant`, addressed to the participant an id names in the stage an ARN
//! names, and `RevokeConnectionToken`, addressed to the token that connection was opened
//! with.
//!
//! Every case goes through the router's own `dispatch` with a restJson1 request, exactly
//! as a signed `aws ivs-realtime disconnect-participant` call arrives, so what is asserted
//! is the emulator's HTTP behaviour and never a handler called directly.
//!
//! The RULES under test, each exercised over a SET of inputs rather than on one request:
//!
//! 1. A disconnect takes the named participant OUT of the stage it is attached to: its
//!    record stays in the session and reports `DISCONNECTED`, every other participant of
//!    that session is untouched, and a session the departure empties ends (the stage
//!    reports no active session again). The answer is the modelled 200 carrying no
//!    member, and a second disconnect of the same participant changes nothing further.
//! 2. "If the participant is publishing using an IngestConfiguration,
//!    DisconnectParticipant also updates the stageArn in the IngestConfiguration to be an
//!    empty string": the configuration the disconnected participant was publishing with
//!    is left associated with no stage, so a read of it reports the empty `stageArn` and
//!    `INACTIVE`. A configuration the disconnected participant was NOT publishing with
//!    keeps its stage.
//! 3. A request naming a stage this account does not hold is the modelled
//!    `ResourceNotFoundException` (HTTP 404) and writes nothing; a value outside a
//!    member's modelled `@pattern` / `@length` is the modelled `ValidationException`
//!    (HTTP 400). A well-formed `participantId` the resolved stage holds no connection for
//!    is NOT a refusal: there is nothing to disconnect, so the call answers the modelled
//!    200 carrying no member and writes nothing — the disconnect is idempotent.
//! 4. Revoking a connection token "so that it can no longer connect to a stage"
//!    disconnects "any participant currently connected with the revoked token": the
//!    holder of the revoked token leaves the stage the token was issued against exactly as
//!    a disconnect leaves it, every other participant and token of that stage is untouched,
//!    and the revoked token is no longer an identity of the stage — so a call that acts on
//!    the stage's publisher under that id no longer finds one. A `tokenId` this region's
//!    view of the account holds no token for names nothing to revoke, so the call answers
//!    the modelled 200 carrying no member and writes nothing, and revoking the same token
//!    again answers the way the first call did.
//!
//! Nothing in the IVS RealTime control plane makes a participant JOIN a stage: a
//! participant joins over the stage's data plane with the participant token or ingest key
//! it was issued, which is `state::join_stage_session` — the one place a participant
//! record is created. So these cases bring participants into a session through it,
//! reaching the store through this suite's own engine handle, exactly as the
//! `ListParticipants` and `DeleteIngestConfiguration` suites beside this one do.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The region every call that does not vary it is signed for.
const REGION: &str = "us-east-1";

/// The two values the modelled `ParticipantState` carries, from
/// `smithy_query(query="shape",shape="ParticipantState")`.
const PARTICIPANT_CONNECTED: &str = "CONNECTED";
const PARTICIPANT_DISCONNECTED: &str = "DISCONNECTED";

/// The two values the modelled `IngestConfigurationState` carries, from
/// `smithy_query(query="shape",shape="IngestConfigurationState")`: a configuration is
/// `ACTIVE` while a publisher is publishing to its stage, and `INACTIVE` otherwise.
const PUBLISHING_STATE: &str = "ACTIVE";
const NO_PUBLISHER_STATE: &str = "INACTIVE";

/// The published `@length` bounds of `ConnectionTokenId` — the member
/// `RevokeConnectionToken` names a token by — from
/// `smithy_query(query="shape",shape="ConnectionTokenId")`: `@length(1..64)`, pattern
/// `^[a-zA-Z0-9-_]+$`.
const TOKEN_ID_MIN_LENGTH: usize = 1;
const TOKEN_ID_MAX_LENGTH: usize = 64;

/// The published `@length` bounds of `DisconnectParticipantReason`, from
/// `smithy_query(query="shape",shape="DisconnectParticipantReason")`.
const REASON_MIN_LENGTH: usize = 0;
const REASON_MAX_LENGTH: usize = 128;

/// The `stageArn` an ingest configuration associated with no stage reports — the empty
/// string the operation's own doc names.
const NO_ASSOCIATED_STAGE_ARN: &str = "";

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches
/// it, plus the engine handle the data-plane seeding writes through.
struct RealTime {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl RealTime {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential
    /// scope naming `ivs`, signed for [`REGION`].
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in(REGION, operation, body).await
    }

    /// [`Self::call`] signed for `region`, so a case can address another region's
    /// endpoint: every resource of this service belongs to the region whose endpoint
    /// created it.
    async fn call_in(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{region}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://ivsrealtime.{region}.amazonaws.com/{operation}"
            ))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create a stage and return the ARN the service minted for it.
    async fn create_stage(&self, case: &str, name: &str) -> String {
        self.create_stage_in(case, REGION, name).await
    }

    /// [`Self::create_stage`] through `region`'s endpoint.
    async fn create_stage_in(&self, case: &str, region: &str, name: &str) -> String {
        let (status, response) = self
            .call_in(region, "CreateStage", &json!({ "name": name }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage {name:?} in {region} must answer HTTP 200, \
             got {status} {response}"
        );
        member(&response["stage"], "arn")
    }

    /// Issue a participant token for `stage_arn` and return the participant id it
    /// carries — the id the holder of that token joins the stage under, and the id of the
    /// token itself.
    async fn create_token(&self, case: &str, stage_arn: &str, user_id: Option<&str>) -> String {
        self.create_token_in(case, REGION, stage_arn, user_id).await
    }

    /// [`Self::create_token`] through `region`'s endpoint.
    async fn create_token_in(
        &self,
        case: &str,
        region: &str,
        stage_arn: &str,
        user_id: Option<&str>,
    ) -> String {
        let mut request = json!({ "stageArn": stage_arn });
        if let Some(user_id) = user_id {
            request["userId"] = json!(user_id);
        }
        let (status, response) = self
            .call_in(region, "CreateParticipantToken", &request)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateParticipantToken {request} in {region} must answer HTTP 200, \
             got {status} {response}"
        );
        member(&response["participantToken"], "participantId")
    }

    /// Create an ingest configuration and return the `ingestConfiguration` the service
    /// reported for it.
    async fn create_ingest_configuration(&self, case: &str, body: &Value) -> Value {
        let (status, response) = self.call("CreateIngestConfiguration", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateIngestConfiguration {body} must answer HTTP 200, \
             got {status} {response}"
        );
        response["ingestConfiguration"].clone()
    }

    /// The `ingestConfiguration` an ARN names, read back over the wire.
    async fn read_ingest_configuration(&self, case: &str, arn: &str) -> Value {
        let (status, response) = self
            .call("GetIngestConfiguration", &json!({ "arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetIngestConfiguration {arn} must answer HTTP 200, \
             got {status} {response}"
        );
        response["ingestConfiguration"].clone()
    }

    /// The stage's `activeSessionId`, read back over the wire (the empty string once the
    /// stage has no active session).
    async fn active_session_id(&self, case: &str, stage_arn: &str) -> String {
        let (status, response) = self.call("GetStage", &json!({ "arn": stage_arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetStage {stage_arn} must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "activeSessionId")
    }

    /// Record a participant joining `stage_arn`, the way the stage's data plane does.
    fn join(&self, case: &str, stage_arn: &str, joined: state::JoinedParticipant) {
        let participant_id = joined.participant_id.clone();
        let service_state = state::service_state(&*self.engine).expect("this service's store");
        let mut store = service_state.lock();
        assert!(
            state::join_stage_session(&mut store, stage_arn, joined).is_some(),
            "case {case:?}: the join of {participant_id:?} on {stage_arn:?} must be recorded"
        );
    }

    /// The state `ListParticipants` reports for one participant of one session, or `None`
    /// when the session holds no record for it.
    async fn participant_state(
        &self,
        case: &str,
        stage_arn: &str,
        session_id: &str,
        participant_id: &str,
    ) -> Option<String> {
        let (status, response) = self
            .call(
                "ListParticipants",
                &json!({ "stageArn": stage_arn, "sessionId": session_id }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListParticipants on {session_id} must answer HTTP 200, \
             got {status} {response}"
        );
        response["participants"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .find(|participant| member(participant, "participantId") == participant_id)
            .map(|participant| member(participant, "state"))
    }

    /// `DisconnectParticipant` with `reason` as the case varies it (absent when `None`).
    async fn disconnect(
        &self,
        stage_arn: &str,
        participant_id: &str,
        reason: Option<&str>,
    ) -> (u16, Value) {
        let mut request = json!({ "stageArn": stage_arn, "participantId": participant_id });
        if let Some(reason) = reason {
            request["reason"] = json!(reason);
        }
        self.call("DisconnectParticipant", &request).await
    }

    /// `RevokeConnectionToken` for the token `token_id` names, signed for [`REGION`].
    async fn revoke(&self, token_id: &str) -> (u16, Value) {
        self.revoke_in(REGION, token_id).await
    }

    /// [`Self::revoke`] through `region`'s endpoint.
    async fn revoke_in(&self, region: &str, token_id: &str) -> (u16, Value) {
        self.call_in(region, "RevokeConnectionToken", &json!({ "tokenId": token_id }))
            .await
    }

    /// `StartParticipantReplication` of `participant_id` from one stage into another — the
    /// call that acts on a stage's PUBLISHER under that id, so what it answers says whether
    /// the stage still has a publishing participant under it.
    async fn start_replication(
        &self,
        source_stage_arn: &str,
        destination_stage_arn: &str,
        participant_id: &str,
    ) -> (u16, Value) {
        self.start_replication_in(REGION, source_stage_arn, destination_stage_arn, participant_id)
            .await
    }

    /// [`Self::start_replication`] through `region`'s endpoint.
    async fn start_replication_in(
        &self,
        region: &str,
        source_stage_arn: &str,
        destination_stage_arn: &str,
        participant_id: &str,
    ) -> (u16, Value) {
        self.call_in(
            region,
            "StartParticipantReplication",
            &json!({
                "sourceStageArn": source_stage_arn,
                "destinationStageArn": destination_stage_arn,
                "participantId": participant_id,
            }),
        )
        .await
    }
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
}

/// A string member, read as the empty string when absent — what a client sees for a
/// member the wire omits.
fn member(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Does this answer carry no members, as the modelled `DisconnectParticipantResponse`
/// (a structure with no members) requires?
fn carries_no_members(response: &Value) -> bool {
    response.is_null() || response.as_object().is_some_and(serde_json::Map::is_empty)
}

/// One participant a case brings into a session: the name its token carries and whether
/// it publishes to the stage.
struct Joining {
    user_id: &'static str,
    published: bool,
}

/// Issue a token for each entry of `roster` and record it joining `stage_arn`, returning
/// the participant ids in join order and the session the joins opened.
async fn populate(
    emulator: &RealTime,
    case: &str,
    stage_arn: &str,
    roster: &[Joining],
) -> (Vec<String>, String) {
    let mut ids = Vec::new();
    for joining in roster {
        let participant_id = emulator
            .create_token(case, stage_arn, Some(joining.user_id))
            .await;
        emulator.join(
            case,
            stage_arn,
            state::JoinedParticipant {
                participant_id: participant_id.clone(),
                user_id: Some(joining.user_id.to_string()),
                published: joining.published,
                ..Default::default()
            },
        );
        ids.push(participant_id);
    }
    let session_id = emulator.active_session_id(case, stage_arn).await;
    assert!(
        !session_id.is_empty(),
        "case {case:?}: the joins must have opened a session of {stage_arn}"
    );
    (ids, session_id)
}

// ---------------------------------------------------------------------------
// Rule 1 — the named participant leaves the stage, and nothing else does
// ---------------------------------------------------------------------------

/// A disconnect moves the named participant to `DISCONNECTED`, leaves every other
/// participant of the session where it was, and ends a session it emptied — over sessions
/// of different sizes, with the departure at different positions in join order and with
/// the optional `reason` at both published bounds of its `@length`.
#[tokio::test]
async fn disconnect_moves_only_the_named_participant_out_of_the_stage() {
    // Rows differ in how many participants are in the session, which one of them is
    // disconnected (first, middle, last), what it was doing there (publishing or only
    // subscribing) and what `reason` the request carries: absent, the empty string (the
    // member's `@length` minimum) and a reason of its maximum length.
    let max_length_reason = "r".repeat(REASON_MAX_LENGTH);
    let empty_reason = "e".repeat(REASON_MIN_LENGTH);
    let cases: Vec<(&str, Vec<Joining>, usize, Option<&str>)> = vec![
        (
            "a lone subscriber, no reason",
            vec![Joining {
                user_id: "solo",
                published: false,
            }],
            0,
            None,
        ),
        (
            "the publisher of a pair",
            vec![
                Joining {
                    user_id: "publisher",
                    published: true,
                },
                Joining {
                    user_id: "subscriber",
                    published: false,
                },
            ],
            0,
            Some("removed by the moderator"),
        ),
        (
            "the last of three, an empty reason",
            vec![
                Joining {
                    user_id: "one",
                    published: true,
                },
                Joining {
                    user_id: "two",
                    published: false,
                },
                Joining {
                    user_id: "three",
                    published: false,
                },
            ],
            2,
            Some(empty_reason.as_str()),
        ),
        (
            "the middle of four, a reason of the maximum length",
            vec![
                Joining {
                    user_id: "alpha",
                    published: false,
                },
                Joining {
                    user_id: "beta",
                    published: true,
                },
                Joining {
                    user_id: "gamma",
                    published: true,
                },
                Joining {
                    user_id: "delta",
                    published: false,
                },
            ],
            1,
            Some(max_length_reason.as_str()),
        ),
    ];

    for (case, roster, disconnected, reason) in cases {
        let emulator = RealTime::new();
        let stage_arn = emulator.create_stage(case, "disconnect").await;
        let (ids, session_id) = populate(&emulator, case, &stage_arn, &roster).await;
        let target = ids[disconnected].clone();

        let (status, response) = emulator.disconnect(&stage_arn, &target, reason).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for the disconnect of {target:?}, \
             got {status} {response}"
        );
        assert!(
            carries_no_members(&response),
            "case {case:?}: expected an answer carrying no members, got {response}"
        );

        // The disconnected participant is reported DISCONNECTED and is still a
        // participant of the session; every other one is untouched.
        for (position, participant_id) in ids.iter().enumerate() {
            let expected = if position == disconnected {
                PARTICIPANT_DISCONNECTED
            } else {
                PARTICIPANT_CONNECTED
            };
            let actual = emulator
                .participant_state(case, &stage_arn, &session_id, participant_id)
                .await;
            assert_eq!(
                actual.as_deref(),
                Some(expected),
                "case {case:?}: expected {participant_id:?} (position {position}) to be \
                 {expected:?} after the disconnect of {target:?}, got {actual:?}"
            );
        }

        // "A stage session … ends after the last participant leaves the stage": the stage
        // reports the session while anyone is left in it, and no session once the
        // disconnect emptied it.
        let active = emulator.active_session_id(case, &stage_arn).await;
        let expected_active = if roster.len() == 1 { "" } else { &session_id };
        assert_eq!(
            active, expected_active,
            "case {case:?}: expected activeSessionId {expected_active:?} after disconnecting \
             {target:?} of {} participants, got {active:?}",
            roster.len()
        );

        // Repeating the disconnect of a participant that has already left answers the same
        // 200 and changes nothing.
        let (status, response) = emulator.disconnect(&stage_arn, &target, reason).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for a repeated disconnect of {target:?}, \
             got {status} {response}"
        );
        let actual = emulator
            .participant_state(case, &stage_arn, &session_id, &target)
            .await;
        assert_eq!(
            actual.as_deref(),
            Some(PARTICIPANT_DISCONNECTED),
            "case {case:?}: expected {target:?} still {PARTICIPANT_DISCONNECTED:?} after a \
             repeated disconnect, got {actual:?}"
        );
        let active_again = emulator.active_session_id(case, &stage_arn).await;
        assert_eq!(
            active_again, active,
            "case {case:?}: expected a repeated disconnect to leave activeSessionId \
             {active:?}, got {active_again:?}"
        );
    }
}

/// Disconnecting every participant in turn ends the session and leaves each record in it,
/// reported `DISCONNECTED` — over sessions of different sizes.
#[tokio::test]
async fn disconnecting_every_participant_ends_the_session_and_keeps_the_records() {
    for size in [1usize, 2, 5] {
        let case = format!("{size} participants disconnected in turn");
        let case = case.as_str();
        let emulator = RealTime::new();
        let stage_arn = emulator.create_stage(case, "drain").await;
        let roster: Vec<Joining> = (0..size)
            .map(|position| Joining {
                // Half the roster publishes, so the drained session spans both.
                user_id: if position % 2 == 0 { "even" } else { "odd" },
                published: position % 2 == 0,
            })
            .collect();
        let (ids, session_id) = populate(&emulator, case, &stage_arn, &roster).await;

        for (position, participant_id) in ids.iter().enumerate() {
            let (status, response) = emulator.disconnect(&stage_arn, participant_id, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 disconnecting {participant_id:?} \
                 (position {position}), got {status} {response}"
            );
            let active = emulator.active_session_id(case, &stage_arn).await;
            let still_connected = position + 1 < ids.len();
            let expected_active = if still_connected { &session_id } else { "" };
            assert_eq!(
                active, expected_active,
                "case {case:?}: expected activeSessionId {expected_active:?} with \
                 {} of {} participants disconnected, got {active:?}",
                position + 1,
                ids.len()
            );
        }

        for participant_id in &ids {
            let actual = emulator
                .participant_state(case, &stage_arn, &session_id, participant_id)
                .await;
            assert_eq!(
                actual.as_deref(),
                Some(PARTICIPANT_DISCONNECTED),
                "case {case:?}: expected {participant_id:?} to stay in the ended session as \
                 {PARTICIPANT_DISCONNECTED:?}, got {actual:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2 — a configuration the disconnected participant published with loses its stage
// ---------------------------------------------------------------------------

/// Which identity the publisher of the ingest configuration broadcasts under.
enum PublisherIdentity {
    /// The configuration's own `participantId`.
    PrimaryCredential,
    /// The `participantId` of one of its `redundantIngestCredentials`.
    RedundantCredential,
    /// Its own `participantId`, with the join also recording the configuration's ARN on
    /// the participant.
    PrimaryCredentialRecordingTheArn,
}

/// A disconnect of a participant publishing with an ingest configuration leaves that
/// configuration associated with no stage — whichever of its credentials the publisher
/// broadcast under, and whether or not the join recorded the configuration's ARN.
#[tokio::test]
async fn disconnecting_an_ingest_publisher_clears_the_configurations_stage() {
    let cases = [
        (
            "the primary ingest credential",
            PublisherIdentity::PrimaryCredential,
            false,
        ),
        (
            "a redundant ingest credential",
            PublisherIdentity::RedundantCredential,
            true,
        ),
        (
            "the primary credential, recorded with the configuration ARN",
            PublisherIdentity::PrimaryCredentialRecordingTheArn,
            false,
        ),
    ];

    for (case, identity, redundant_ingest) in cases {
        let emulator = RealTime::new();
        let stage_arn = emulator.create_stage(case, "ingest").await;
        let configuration = emulator
            .create_ingest_configuration(
                case,
                &json!({
                    "ingestProtocol": "RTMPS",
                    "stageArn": stage_arn,
                    "insecureIngest": false,
                    "redundantIngest": redundant_ingest,
                }),
            )
            .await;
        let configuration_arn = member(&configuration, "arn");

        let (participant_id, configuration_of_join) = match identity {
            PublisherIdentity::PrimaryCredential => (member(&configuration, "participantId"), None),
            PublisherIdentity::RedundantCredential => (
                member(
                    &configuration["redundantIngestCredentials"][0],
                    "participantId",
                ),
                None,
            ),
            PublisherIdentity::PrimaryCredentialRecordingTheArn => (
                member(&configuration, "participantId"),
                Some(configuration_arn.clone()),
            ),
        };
        assert!(
            !participant_id.is_empty(),
            "case {case:?}: the publishing participant must be identified, got {configuration}"
        );
        emulator.join(
            case,
            &stage_arn,
            state::JoinedParticipant {
                participant_id: participant_id.clone(),
                published: true,
                redundant_ingest,
                ingest_configuration_arn: configuration_of_join,
                ..Default::default()
            },
        );
        let session_id = emulator.active_session_id(case, &stage_arn).await;

        // While the publisher is broadcasting, the configuration names the stage and is
        // reported ACTIVE.
        let read = emulator
            .read_ingest_configuration(case, &configuration_arn)
            .await;
        assert_eq!(
            member(&read, "stageArn"),
            stage_arn,
            "case {case:?}: expected the publishing configuration to name {stage_arn}, got {read}"
        );
        assert_eq!(
            member(&read, "state"),
            PUBLISHING_STATE,
            "case {case:?}: expected state {PUBLISHING_STATE:?} while the publisher is \
             broadcasting, got {read}"
        );

        let (status, response) = emulator
            .disconnect(&stage_arn, &participant_id, Some("ingest shut down"))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 disconnecting the publisher, got {status} {response}"
        );

        // The publisher has left the stage …
        let actual = emulator
            .participant_state(case, &stage_arn, &session_id, &participant_id)
            .await;
        assert_eq!(
            actual.as_deref(),
            Some(PARTICIPANT_DISCONNECTED),
            "case {case:?}: expected the publisher {PARTICIPANT_DISCONNECTED:?} after the \
             disconnect, got {actual:?}"
        );
        // … and its configuration is associated with no stage, so it reports no publisher.
        let read = emulator
            .read_ingest_configuration(case, &configuration_arn)
            .await;
        assert_eq!(
            member(&read, "stageArn"),
            NO_ASSOCIATED_STAGE_ARN,
            "case {case:?}: expected the disconnected publisher's configuration to name no \
             stage, got {read}"
        );
        assert_eq!(
            member(&read, "state"),
            NO_PUBLISHER_STATE,
            "case {case:?}: expected state {NO_PUBLISHER_STATE:?} once the publisher was \
             disconnected, got {read}"
        );
        // The configuration itself survives, ingest key included: only its stage changed.
        assert_eq!(
            member(&read, "streamKey"),
            member(&configuration, "streamKey"),
            "case {case:?}: expected the configuration to keep its ingest key, got {read}"
        );
    }
}

/// A disconnect leaves a configuration the disconnected participant was NOT publishing
/// with exactly as it was — whether that configuration's publisher is in the same stage or
/// the configuration is attached to another stage entirely.
#[tokio::test]
async fn disconnecting_another_participant_leaves_a_configuration_alone() {
    for case in [
        "a token holder of the publisher's own stage",
        "a publisher of another stage",
    ] {
        let emulator = RealTime::new();
        let stage_arn = emulator.create_stage(case, "publishing").await;
        let other_stage_arn = emulator.create_stage(case, "elsewhere").await;
        let configuration = emulator
            .create_ingest_configuration(
                case,
                &json!({ "ingestProtocol": "RTMP", "insecureIngest": true, "stageArn": stage_arn }),
            )
            .await;
        let configuration_arn = member(&configuration, "arn");
        let publisher_id = member(&configuration, "participantId");
        emulator.join(
            case,
            &stage_arn,
            state::JoinedParticipant {
                participant_id: publisher_id.clone(),
                published: true,
                ingest_configuration_arn: Some(configuration_arn.clone()),
                ..Default::default()
            },
        );

        // The participant this case disconnects: one holding a token of the publisher's
        // stage, or the publisher of a second stage of the same account.
        let (disconnect_from, disconnected_id) = if case.contains("another stage") {
            let other_configuration = emulator
                .create_ingest_configuration(
                    case,
                    &json!({
                        "ingestProtocol": "RTMP",
                        "insecureIngest": true,
                        "stageArn": other_stage_arn,
                    }),
                )
                .await;
            let other_publisher = member(&other_configuration, "participantId");
            emulator.join(
                case,
                &other_stage_arn,
                state::JoinedParticipant {
                    participant_id: other_publisher.clone(),
                    published: true,
                    ingest_configuration_arn: Some(member(&other_configuration, "arn")),
                    ..Default::default()
                },
            );
            (other_stage_arn.clone(), other_publisher)
        } else {
            let token_holder = emulator.create_token(case, &stage_arn, Some("viewer")).await;
            emulator.join(
                case,
                &stage_arn,
                state::JoinedParticipant {
                    participant_id: token_holder.clone(),
                    user_id: Some("viewer".to_string()),
                    ..Default::default()
                },
            );
            (stage_arn.clone(), token_holder)
        };

        let (status, response) = emulator
            .disconnect(&disconnect_from, &disconnected_id, None)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 disconnecting {disconnected_id:?}, \
             got {status} {response}"
        );

        let read = emulator
            .read_ingest_configuration(case, &configuration_arn)
            .await;
        assert_eq!(
            member(&read, "stageArn"),
            stage_arn,
            "case {case:?}: expected the untouched configuration to still name {stage_arn}, \
             got {read}"
        );
        assert_eq!(
            member(&read, "state"),
            PUBLISHING_STATE,
            "case {case:?}: expected the untouched configuration to stay {PUBLISHING_STATE:?} \
             while its own publisher broadcasts, got {read}"
        );
        let session_id = emulator.active_session_id(case, &stage_arn).await;
        let actual = emulator
            .participant_state(case, &stage_arn, &session_id, &publisher_id)
            .await;
        assert_eq!(
            actual.as_deref(),
            Some(PARTICIPANT_CONNECTED),
            "case {case:?}: expected the configuration's own publisher to stay \
             {PARTICIPANT_CONNECTED:?}, got {actual:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3 — what is refused, and that a refusal writes nothing
// ---------------------------------------------------------------------------

/// A request naming a stage this account does not hold is refused with
/// `ResourceNotFoundException` (404), and a member outside its modelled constraints with
/// `ValidationException` (400); a well-formed participant id the resolved stage holds no
/// connection for is the idempotent empty 200 instead. Neither answer moves the
/// participant that IS in the stage.
#[tokio::test]
async fn a_request_naming_no_resource_of_this_account_is_refused() {
    let emulator = RealTime::new();
    let stage_arn = emulator.create_stage("refusals", "held").await;
    let other_stage_arn = emulator.create_stage("refusals", "other").await;
    let (ids, session_id) = populate(
        &emulator,
        "refusals",
        &stage_arn,
        &[Joining {
            user_id: "resident",
            published: true,
        }],
    )
    .await;
    let resident = ids[0].clone();
    // A participant of the OTHER stage: a well-formed id this stage has no identity for.
    let elsewhere = emulator
        .create_token("refusals", &other_stage_arn, Some("guest"))
        .await;
    // A stage ARN of the same account and region, naming a stage nothing created.
    let absent_stage_arn = {
        let (prefix, _) = stage_arn
            .rsplit_once('/')
            .expect("a stage ARN names its resource");
        format!("{prefix}/absent")
    };

    // Rows differ in WHICH identifier does not resolve, and in how: an unheld stage and a
    // `stageArn` outside the member's modelled pattern are refused, while an id no token
    // or ingest credential of this stage was minted for (including the empty id the
    // member's `@length` minimum admits, and one belonging to another stage) names no
    // connection to end and is answered with the empty 200.
    //
    // (case, stageArn, participantId, status, the error code — empty where the row is a
    // success)
    let cases: Vec<(&str, String, String, u16, &str)> = vec![
        (
            "a stage this account does not hold",
            absent_stage_arn.clone(),
            resident.clone(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a participant id nothing was minted for",
            stage_arn.clone(),
            "no-such-participant".to_string(),
            200,
            "",
        ),
        (
            "the empty participant id",
            stage_arn.clone(),
            String::new(),
            200,
            "",
        ),
        (
            "a participant of another stage",
            stage_arn.clone(),
            elsewhere.clone(),
            200,
            "",
        ),
        (
            "a stageArn outside the modelled pattern",
            "not-a-stage-arn".to_string(),
            resident.clone(),
            400,
            "ValidationException",
        ),
    ];

    for (case, requested_stage, requested_participant, expected_status, expected_code) in cases {
        let (status, response) = emulator
            .disconnect(&requested_stage, &requested_participant, Some("why"))
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for stageArn \
             {requested_stage:?} / participantId {requested_participant:?}, \
             got {status} {response}"
        );
        if expected_code.is_empty() {
            // The modelled output carries no member at all, so the whole answer is the
            // empty JSON object.
            assert_eq!(
                response,
                json!({}),
                "case {case:?}: expected an answer carrying no member, got {response}"
            );
        } else {
            assert_eq!(
                error_code(&response),
                expected_code,
                "case {case:?}: expected {expected_code}, got {response}"
            );
        }
        // Neither the refusal nor the no-op wrote anything: the stage's own participant is
        // still connected.
        let actual = emulator
            .participant_state("refusals", &stage_arn, &session_id, &resident)
            .await;
        assert_eq!(
            actual.as_deref(),
            Some(PARTICIPANT_CONNECTED),
            "case {case:?}: expected the call to leave {resident:?} \
             {PARTICIPANT_CONNECTED:?}, got {actual:?}"
        );
    }

    // And the stage that was never touched still reports its own session.
    let active = emulator
        .active_session_id("refusals", &stage_arn)
        .await;
    assert_eq!(
        active, session_id,
        "expected the stage to keep its session {session_id:?} across every refusal, \
         got {active:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4 — revoking a connection token ends the connection it opened, and the
// token admits nobody afterwards
// ---------------------------------------------------------------------------

/// Revoking a token "disconnects any participant currently connected with the revoked
/// token": that holder's record moves to `DISCONNECTED`, every other holder of the same
/// session stays where it was, and a session the departure empties ends — over sessions of
/// different sizes, with the revoked token at different positions in join order and with
/// its holder publishing or only subscribing.
#[tokio::test]
async fn revoking_a_token_takes_only_its_holder_out_of_the_stage() {
    // Rows differ in how many token holders are in the session, WHICH of their tokens is
    // revoked (first, middle, last) and what that holder was doing in the stage.
    let cases: Vec<(&str, Vec<Joining>, usize)> = vec![
        (
            "a lone subscriber",
            vec![Joining {
                user_id: "solo",
                published: false,
            }],
            0,
        ),
        (
            "a lone publisher",
            vec![Joining {
                user_id: "broadcaster",
                published: true,
            }],
            0,
        ),
        (
            "the publisher of a pair",
            vec![
                Joining {
                    user_id: "publisher",
                    published: true,
                },
                Joining {
                    user_id: "subscriber",
                    published: false,
                },
            ],
            0,
        ),
        (
            "the subscriber of a pair",
            vec![
                Joining {
                    user_id: "publisher",
                    published: true,
                },
                Joining {
                    user_id: "subscriber",
                    published: false,
                },
            ],
            1,
        ),
        (
            "the middle of three",
            vec![
                Joining {
                    user_id: "one",
                    published: true,
                },
                Joining {
                    user_id: "two",
                    published: false,
                },
                Joining {
                    user_id: "three",
                    published: false,
                },
            ],
            1,
        ),
        (
            "the last of three",
            vec![
                Joining {
                    user_id: "one",
                    published: true,
                },
                Joining {
                    user_id: "two",
                    published: false,
                },
                Joining {
                    user_id: "three",
                    published: true,
                },
            ],
            2,
        ),
    ];

    for (case, roster, revoked_index) in cases {
        let emulator = RealTime::new();
        let stage_arn = emulator.create_stage(case, "revoked").await;
        let (ids, session_id) = populate(&emulator, case, &stage_arn, &roster).await;
        let revoked = ids[revoked_index].clone();

        let (status, response) = emulator.revoke(&revoked).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 revoking {revoked:?}, got {status} {response}"
        );
        assert!(
            carries_no_members(&response),
            "case {case:?}: expected an answer carrying no member, got {response}"
        );

        for (index, participant_id) in ids.iter().enumerate() {
            let expected = if index == revoked_index {
                PARTICIPANT_DISCONNECTED
            } else {
                PARTICIPANT_CONNECTED
            };
            let actual = emulator
                .participant_state(case, &stage_arn, &session_id, participant_id)
                .await;
            assert_eq!(
                actual.as_deref(),
                Some(expected),
                "case {case:?}: expected {participant_id:?} {expected:?} after revoking \
                 {revoked:?}, got {actual:?}"
            );
        }

        // A session the departure emptied ends, so the stage reports no active session
        // again; one that still holds somebody keeps the session it had.
        let expected_active = if roster.len() == 1 {
            String::new()
        } else {
            session_id.clone()
        };
        let active = emulator.active_session_id(case, &stage_arn).await;
        assert_eq!(
            active, expected_active,
            "case {case:?}: expected activeSessionId {expected_active:?} after revoking \
             {revoked:?}, got {active:?}"
        );
    }
}

/// A revoked token "can no longer connect to a stage": it is no longer one of the
/// identities this service issued against that stage, so a call that acts on the stage's
/// publisher under that id finds none — while a token of the same stage that nothing
/// revoked still names one.
///
/// The rows differ in what the token's holder had done with it by the time it was revoked:
/// nothing at all, joined to subscribe, or joined and published.
#[tokio::test]
async fn a_revoked_token_is_no_longer_an_identity_of_the_stage() {
    for (case, joins, publishes) in [
        ("a token nobody connected with", false, false),
        ("a token whose holder subscribes", true, false),
        ("a token whose holder publishes", true, true),
    ] {
        let emulator = RealTime::new();
        let stage_arn = emulator.create_stage(case, "source").await;
        let destination_stage_arn = emulator.create_stage(case, "destination").await;
        // Two tokens of the same stage, issued alike and used alike: one is revoked and
        // one is left alone, so the only difference between what the two answer is the
        // revocation.
        let revoked = emulator.create_token(case, &stage_arn, Some("revoked")).await;
        let kept = emulator.create_token(case, &stage_arn, Some("kept")).await;
        if joins {
            for participant_id in [&revoked, &kept] {
                emulator.join(
                    case,
                    &stage_arn,
                    state::JoinedParticipant {
                        participant_id: participant_id.clone(),
                        published: publishes,
                        ..Default::default()
                    },
                );
            }
        }

        let (status, response) = emulator.revoke(&revoked).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 revoking {revoked:?}, got {status} {response}"
        );

        // The stage publishes nothing under the revoked id any more, which is the modelled
        // `ConflictException` (409) a replication of a non-publishing participant answers.
        let (status, response) = emulator
            .start_replication(&stage_arn, &destination_stage_arn, &revoked)
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409 replicating the revoked {revoked:?}, \
             got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ConflictException",
            "case {case:?}: expected ConflictException for the revoked {revoked:?}, \
             got {response}"
        );

        // The token nothing revoked still names a publishing participant of the stage.
        let (status, response) = emulator
            .start_replication(&stage_arn, &destination_stage_arn, &kept)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 replicating the un-revoked {kept:?}, \
             got {status} {response}"
        );
    }
}

/// Revoking the same token again answers the way the first call did and writes nothing
/// further, and no other token of the account is touched: the other holder of the same
/// stage, and a holder of another stage entirely, stay connected.
#[tokio::test]
async fn revoking_the_same_token_again_changes_nothing() {
    // Rows differ in how many times the one token is revoked.
    for repeats in [1usize, 2, 3] {
        let case = format!("revoked {repeats} time(s)");
        let case = case.as_str();
        let emulator = RealTime::new();
        let stage_arn = emulator.create_stage(case, "revoked").await;
        let other_stage_arn = emulator.create_stage(case, "elsewhere").await;
        let (ids, session_id) = populate(
            &emulator,
            case,
            &stage_arn,
            &[
                Joining {
                    user_id: "revoked",
                    published: true,
                },
                Joining {
                    user_id: "kept",
                    published: false,
                },
            ],
        )
        .await;
        let revoked = ids[0].clone();
        let kept = ids[1].clone();
        let (other_ids, other_session_id) = populate(
            &emulator,
            case,
            &other_stage_arn,
            &[Joining {
                user_id: "stranger",
                published: true,
            }],
        )
        .await;
        let stranger = other_ids[0].clone();

        for round in 1..=repeats {
            let (status, response) = emulator.revoke(&revoked).await;
            assert_eq!(
                status, 200,
                "case {case:?}: round {round} revoking {revoked:?} expected HTTP 200, \
                 got {status} {response}"
            );
            assert!(
                carries_no_members(&response),
                "case {case:?}: round {round} expected an answer carrying no member, \
                 got {response}"
            );

            // Each round leaves exactly what the first one left.
            for (participant_id, stage, session, expected) in [
                (&revoked, &stage_arn, &session_id, PARTICIPANT_DISCONNECTED),
                (&kept, &stage_arn, &session_id, PARTICIPANT_CONNECTED),
                (
                    &stranger,
                    &other_stage_arn,
                    &other_session_id,
                    PARTICIPANT_CONNECTED,
                ),
            ] {
                let actual = emulator
                    .participant_state(case, stage, session, participant_id)
                    .await;
                assert_eq!(
                    actual.as_deref(),
                    Some(expected),
                    "case {case:?}: round {round} expected {participant_id:?} of {stage} \
                     {expected:?}, got {actual:?}"
                );
            }
        }

        // The other token of the revoked token's own stage still names a publishing
        // participant of it.
        let (status, response) = emulator
            .start_replication(&stage_arn, &other_stage_arn, &kept)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 replicating the untouched {kept:?}, \
             got {status} {response}"
        );
    }
}

/// A `tokenId` outside the member's modelled `@length` / `@pattern` is the modelled
/// `ValidationException` (400); a well-formed id this request's own region's view of the
/// account holds no token for names nothing to revoke, so it answers the modelled 200
/// carrying no member. Neither writes anything.
#[tokio::test]
async fn a_revoke_naming_no_token_of_this_region_is_refused_or_already_satisfied() {
    /// A second region, so a case can name a token only another region's endpoint issued.
    const OTHER_REGION: &str = "eu-west-1";

    let emulator = RealTime::new();
    let stage_arn = emulator.create_stage("revoke refusals", "held").await;
    let (ids, session_id) = populate(
        &emulator,
        "revoke refusals",
        &stage_arn,
        &[Joining {
            user_id: "resident",
            published: true,
        }],
    )
    .await;
    let resident = ids[0].clone();
    // A token of another region: issued against a stage that region's endpoint created, so
    // this region's view of the account holds neither.
    let other_region_stage = emulator
        .create_stage_in("revoke refusals", OTHER_REGION, "elsewhere")
        .await;
    let other_region_destination = emulator
        .create_stage_in("revoke refusals", OTHER_REGION, "destination")
        .await;
    let other_region_token = emulator
        .create_token_in("revoke refusals", OTHER_REGION, &other_region_stage, None)
        .await;

    // Rows differ in HOW the id fails to name a token of this region: a well-formed id
    // nothing was minted for (at each published `@length` bound), a token of another
    // region, and a value outside the member's modelled constraints (one past the length
    // maximum, one below the minimum, and outside the pattern).
    //
    // (case, tokenId, status, the error code — empty where the row is a success)
    let cases: Vec<(&str, String, u16, &str)> = vec![
        (
            "an id nothing was minted for",
            "no-such-token".to_string(),
            200,
            "",
        ),
        (
            "an id at the length minimum",
            "a".repeat(TOKEN_ID_MIN_LENGTH),
            200,
            "",
        ),
        (
            "an id at the length maximum",
            "t".repeat(TOKEN_ID_MAX_LENGTH),
            200,
            "",
        ),
        (
            "a token of another region",
            other_region_token.clone(),
            200,
            "",
        ),
        (
            "an id one past the length maximum",
            "t".repeat(TOKEN_ID_MAX_LENGTH + 1),
            400,
            "ValidationException",
        ),
        (
            "the empty id, one below the length minimum",
            String::new(),
            400,
            "ValidationException",
        ),
        (
            "an id outside the modelled pattern",
            "not a token!".to_string(),
            400,
            "ValidationException",
        ),
    ];

    for (case, token_id, expected_status, expected_code) in cases {
        let (status, response) = emulator.revoke(&token_id).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} revoking {token_id:?}, \
             got {status} {response}"
        );
        if expected_code.is_empty() {
            assert!(
                carries_no_members(&response),
                "case {case:?}: expected an answer carrying no member, got {response}"
            );
        } else {
            assert_eq!(
                error_code(&response),
                expected_code,
                "case {case:?}: expected {expected_code}, got {response}"
            );
        }
        // Nothing was written: this region's own participant is still connected.
        let actual = emulator
            .participant_state(case, &stage_arn, &session_id, &resident)
            .await;
        assert_eq!(
            actual.as_deref(),
            Some(PARTICIPANT_CONNECTED),
            "case {case:?}: expected the call to leave {resident:?} {PARTICIPANT_CONNECTED:?}, \
             got {actual:?}"
        );
    }

    // And the other region's token still names a publishing participant of its own stage:
    // the call addressed to this region revoked nothing of that one.
    let (status, response) = emulator
        .start_replication_in(
            OTHER_REGION,
            &other_region_stage,
            &other_region_destination,
            &other_region_token,
        )
        .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 replicating {other_region_token:?} in {OTHER_REGION}, \
         got {status} {response}"
    );
}
