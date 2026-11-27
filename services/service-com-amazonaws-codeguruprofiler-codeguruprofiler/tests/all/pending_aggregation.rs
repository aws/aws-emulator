// SPDX-License-Identifier: CC0-1.0

//! What a group's profile surface reports while the aggregation a posted agent profile
//! is owed has not fallen due yet, end to end over HTTP through the router this service
//! mounts in production.
//!
//! The rule every case here asserts: `GetProfile` and `ListProfileTimes` report
//! AGGREGATED profiles, and a posted agent profile is not one at the instant it is
//! received — the service collects posted profiles into aggregated profiles at periods
//! aligned to UTC, and its pass over a window runs once that window has closed and the
//! submission has arrived, so nothing a post just delivered is aggregated when the post
//! returns. Until the pass is through the window is covered by no aggregated profile, so
//! `GetProfile` answers its 404 `ResourceNotFoundException` with the fixed no-data text
//! and `ListProfileTimes` reports no profile time for it — while the group still records
//! that the profile was RECEIVED
//! (`profilingStatus.latestAgentProfileReportedAt`). Once the aggregation is due, the
//! same requests serve the aggregated profile the window's profiles were collected into —
//! each period's own aggregated profile once the window OF THAT PERIOD has closed.
//!
//! Rows vary what the rule generalises over: how the requested window is expressed (no
//! parameters, each of the three modelled aggregation periods alone, both bounds, each
//! bound with the window's length), the payload's size and bytes, the media type it was
//! posted under, and how many profiles the group holds.
//!
//! The second half of that rule is the one the cases from
//! [`a_submission_covering_a_closed_window_is_retrievable_once_the_pass_has_run`] on
//! assert, with the clock alone deciding it: a submission whose data covers a window that
//! had ALREADY closed when it was posted is owed its aggregated profile as soon as the
//! pass over the submission itself is through, so `GetProfile` for the matching period
//! then answers 200 with that window's aggregated document — and whatever it answers, it
//! answers: the 200 payload or one of the operation's modelled errors, never a request
//! left hanging.
//!
//! The harness is the one [`crate::profiling_group_lifecycle`] already builds, the
//! aggregation is let fall due through the one shared helper every suite uses, and the
//! clock is read clear of every window boundary through
//! [`crate::profile_times::now_clear_of_window_boundaries`].

use crate::profile_times::now_clear_of_window_boundaries;
use crate::profiling_group_lifecycle::{
    Answer, Harness, aggregation_of, data_of, element_of, ion_profile, iso_millis, json_profile,
    pending_profile_start_millis, profile_data, servable_window, servable_window_from_start,
    servable_window_to_end,
};

/// The fixed text a 404 about absent profile DATA carries
/// (`state::NO_PROFILES_IN_TIME_RANGE`), restated here so this suite pins the wire text
/// rather than importing the value it compares against.
const NO_PROFILES_IN_TIME_RANGE: &str = "No profiles found for the requested time range!";

/// A window wide enough to contain any instant these cases place a receipt at, in the
/// ISO 8601 form the modelled `date-time` query members carry.
const WIDE_START: &str = "2000-01-01T00:00:00Z";
const WIDE_END: &str = "2099-01-01T00:00:00Z";

/// The modelled `AggregationPeriod` values the service aggregates at, with each period's
/// length in whole seconds, read from the service's own table (finest window first) so this
/// suite cannot name a period the service does not aggregate at.
const AGGREGATION_PERIODS: &[(&str, i64)] =
    service_com_amazonaws_codeguruprofiler_codeguruprofiler::state::AGGREGATION_PERIODS;

async fn create_group(harness: &Harness, name: &str) -> Answer {
    harness
        .send(
            "POST",
            &format!("/profilingGroups?clientToken=tok-{name}"),
            "application/json",
            serde_json::to_vec(&serde_json::json!({
                "profilingGroupName": name,
                "agentOrchestrationConfig": { "profilingEnabled": true }
            }))
            .expect("a serializable body"),
        )
        .await
}

async fn list_profile_times(harness: &Harness, name: &str, period: &str) -> Answer {
    harness
        .send(
            "GET",
            &format!(
                "/profilingGroups/{name}/profileTimes?startTime={WIDE_START}&endTime={WIDE_END}\
                 &period={period}"
            ),
            "application/json",
            Vec::new(),
        )
        .await
}

/// How many profile times one `ListProfileTimes` answer reports.
fn reported_times(answer: &Answer) -> usize {
    answer
        .json()
        .get("profileTimes")
        .and_then(serde_json::Value::as_array)
        .map(Vec::len)
        .unwrap_or_else(|| panic!("no profileTimes array in {}", answer.json()))
}

/// One row: how the `GetProfile` window is expressed, and the profile posted under it.
struct Row {
    label: &'static str,
    /// The `GetProfile` query string, which must select the stored profile once its
    /// aggregation is due.
    query: String,
    content_type: &'static str,
    /// How many bytes of padding the posted profile carries, which is what makes the rows
    /// differ in payload size.
    filler: usize,
    /// How many profiles the group is posted, each with its own `profileToken`.
    posts: usize,
}

impl Row {
    /// The bytes of post `post`: a profile stating its own `start`, in the representation
    /// this row posts under, so no two posts of a row carry the same bytes.
    ///
    /// The period stated is the one the service is still collecting into, which is what
    /// makes the aggregation this suite is about PENDING.
    fn posted_bytes(&self, post: usize) -> Vec<u8> {
        let start = pending_profile_start_millis() + i64::try_from(post).unwrap_or(0);
        if self.content_type == "application/x-amzn-ion" {
            ion_profile(start, self.filler)
        } else {
            json_profile(start, self.filler)
        }
    }
}

fn rows() -> Vec<Row> {
    // The three period values are the ones the model's `AggregationPeriod` enum declares
    // (PT5M, PT1H, P1D); the media types are the two the model's `GetProfile$contentType`
    // doc names ("either application/json or the default application/x-amzn-ion").
    vec![
        Row {
            label: "no parameters",
            query: String::new(),
            content_type: "application/json",
            filler: 0,
            posts: 1,
        },
        Row {
            label: "period PT5M alone",
            query: "?period=PT5M".to_string(),
            content_type: "application/json",
            filler: 1,
            posts: 1,
        },
        Row {
            label: "period PT1H alone",
            query: "?period=PT1H".to_string(),
            content_type: "application/x-amzn-ion",
            filler: 255,
            posts: 2,
        },
        Row {
            label: "period P1D alone",
            query: "?period=P1D".to_string(),
            content_type: "application/x-amzn-ion",
            filler: 4_096,
            posts: 1,
        },
        Row {
            label: "both bounds",
            // A `GetProfile` pair of bounds is bounded by the maximum range the
            // operation's doc allows, so the window is the shared one that holds a
            // just-posted receipt and stays inside that limit
            // ([`crate::get_profile_window_length`] owns the rule over its width).
            query: servable_window(),
            content_type: "application/json",
            filler: 24,
            posts: 3,
        },
        Row {
            label: "a start bound and a length",
            // A `GetProfile` bound must be accompanied by the other bound or by the
            // window's length, so a servable window named from ONE bound names the length
            // too ([`crate::get_profile_window_companion`] owns that rule).
            query: servable_window_from_start(),
            content_type: "application/json",
            filler: 0,
            posts: 1,
        },
        Row {
            label: "an end bound and a length",
            query: servable_window_to_end(),
            content_type: "application/x-amzn-ion",
            filler: 0,
            posts: 2,
        },
    ]
}

#[tokio::test]
async fn a_posted_profile_is_served_only_once_its_aggregation_is_due() {
    let harness = Harness::new();

    for (index, row) in rows().iter().enumerate() {
        let name = format!("pending-{index}");
        let case = row.label;
        assert_eq!(
            create_group(&harness, &name).await.status,
            201,
            "case {case:?}: create must succeed"
        );

        // Before any post there is nothing to aggregate, so the same 404 answers.
        let before = harness.get_profile(&name, &row.query).await;
        assert_eq!(
            before.status, 404,
            "case {case:?}: GetProfile before any post must answer 404, got {} {}",
            before.status,
            String::from_utf8_lossy(&before.body)
        );

        for post in 0..row.posts {
            let bytes = row.posted_bytes(post);
            assert_eq!(
                harness
                    .post_agent_profile(
                        &name,
                        row.content_type,
                        Some(&format!("ptok-{index}-{post}")),
                        bytes.clone(),
                    )
                    .await
                    .status,
                204,
                "case {case:?}: post {post} must answer 204"
            );
        }

        // The post was RECEIVED — the group says so — and is still not aggregated.
        let described = harness.describe(&name).await;
        assert!(
            described
                .json()
                .pointer("/profilingStatus/latestAgentProfileReportedAt")
                .is_some(),
            "case {case:?}: the post must be recorded as received, got {}",
            String::from_utf8_lossy(&described.body)
        );

        let pending = harness.get_profile(&name, &row.query).await;
        assert_eq!(
            pending.status, 404,
            "case {case:?}: a profile whose aggregation is not due must not be served, got \
             {} {}",
            pending.status,
            String::from_utf8_lossy(&pending.body)
        );
        assert_eq!(
            pending.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            String::from_utf8_lossy(&pending.body)
        );
        assert_eq!(
            pending.error_message(),
            NO_PROFILES_IN_TIME_RANGE,
            "case {case:?}: expected the fixed no-data text, got {:?}",
            pending.error_message()
        );
        // The absence is about the DATA, not about the group: nothing of the request is
        // interpolated into the sentence.
        assert!(
            !pending.error_message().contains(&name),
            "case {case:?}: the no-data text must not name the group, got {:?}",
            pending.error_message()
        );

        // No aggregated profile exists yet, so no profile time is reported for any period.
        for period in ["PT5M", "PT1H", "P1D"] {
            let times = list_profile_times(&harness, &name, period).await;
            assert_eq!(
                times.status, 200,
                "case {case:?} ({period}): ListProfileTimes must answer 200, got {} {}",
                times.status,
                String::from_utf8_lossy(&times.body)
            );
            assert_eq!(
                reported_times(&times),
                0,
                "case {case:?} ({period}): a pending aggregation must report no profile \
                 time, got {}",
                String::from_utf8_lossy(&times.body)
            );
        }

        // Once the aggregation its window owes has fallen due, the SAME request serves
        // the aggregated profile the window's profiles were collected into — so the 404
        // above is about the pending aggregation and not about a profile the store never
        // kept.
        harness.complete_aggregation(&name);
        let served = harness.get_profile(&name, &row.query).await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: an aggregated profile must be served, got {} {}",
            served.status,
            String::from_utf8_lossy(&served.body)
        );
        // A row's posts differ only in the start each profile states, so the DATA the
        // window's profiles combine into is the data of the first of them; the window the
        // served document states is the aggregation's own, not any upload's.
        let expected = profile_data(&row.posted_bytes(0));
        assert_eq!(
            profile_data(&served.body),
            expected,
            "case {case:?}: expected the posted data, got {}",
            element_of(&served.body)
        );
        let (_, _, reported_agents) = aggregation_of(case, &served.body);
        assert_eq!(
            reported_agents,
            i64::try_from(row.posts).unwrap_or(0),
            "case {case:?}: expected the window's {} submissions to be reported, got {}",
            row.posts,
            element_of(&served.body)
        );
        // The aggregation a window owes falls due only once THAT window has closed, so
        // each period's aggregated profile is reported once the window OF THAT PERIOD
        // holding the receipt has closed: the receipt is aged past that window before the
        // period is asked about. The service's table lists the periods finest first, so
        // each step only moves the receipt further into the past.
        for (period, period_seconds) in AGGREGATION_PERIODS {
            harness.complete_aggregation_of_period(&name, *period_seconds);
            let times = list_profile_times(&harness, &name, period).await;
            assert_eq!(
                times.status, 200,
                "case {case:?} ({period}): ListProfileTimes must answer 200, got {} {}",
                times.status,
                String::from_utf8_lossy(&times.body)
            );
            assert!(
                reported_times(&times) >= 1,
                "case {case:?} ({period}): a window of this period whose aggregation has \
                 fallen due must be reported, got {}",
                String::from_utf8_lossy(&times.body)
            );
        }
    }
}

#[tokio::test]
async fn reading_a_pending_aggregation_changes_nothing_a_later_caller_sees() {
    let harness = Harness::new();
    let name = "pending-read";
    assert_eq!(
        create_group(&harness, name).await.status,
        201,
        "create must succeed"
    );
    let bytes = format!(
        "{{\"start\":{},\"frames\":[\"A\"]}}",
        pending_profile_start_millis()
    )
    .into_bytes();
    assert_eq!(
        harness
            .post_agent_profile(name, "application/json", None, bytes.clone())
            .await
            .status,
        204,
        "the post must succeed"
    );
    let reported_at = harness
        .describe(name)
        .await
        .json()
        .pointer("/profilingStatus/latestAgentProfileReportedAt")
        .cloned()
        .expect("the post must be recorded");

    // Polling the pending window is what a client written against the real service does;
    // it must neither serve the profile early nor consume it.
    for attempt in 0..3 {
        let answer = harness.get_profile(name, "?period=PT5M").await;
        assert_eq!(
            answer.status, 404,
            "attempt {attempt}: a pending aggregation must stay pending, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }
    assert_eq!(
        harness
            .describe(name)
            .await
            .json()
            .pointer("/profilingStatus/latestAgentProfileReportedAt")
            .cloned(),
        Some(reported_at),
        "polling GetProfile must not move the recorded profile-report time"
    );

    harness.complete_aggregation(name);
    // And once it is due, two reads of the same window agree: the read is reporting.
    let first = harness.get_profile(name, "?period=PT5M").await;
    let second = harness.get_profile(name, "?period=PT5M").await;
    assert_eq!(
        first.status, 200,
        "the aggregated profile must be served, got {} {}",
        first.status,
        String::from_utf8_lossy(&first.body)
    );
    assert_eq!(
        first.body, second.body,
        "two reads of one aggregated profile must agree"
    );
    assert_eq!(
        profile_data(&first.body),
        profile_data(&bytes),
        "the aggregated profile must hold the posted data, got {}",
        element_of(&first.body)
    );
}

/// The two media types the model's `GetProfile$contentType` doc names ("either
/// application/json or the default application/x-amzn-ion"): the ones a submission is
/// posted under and the ones a request accepts.
const JSON: &str = "application/json";
const ION: &str = "application/x-amzn-ion";

/// The length in whole seconds of the modelled `AggregationPeriod` value `period`, read
/// off the service's own table ([`AGGREGATION_PERIODS`]) so no case can name a period this
/// service does not aggregate at.
fn period_length(period: &str) -> i64 {
    AGGREGATION_PERIODS
        .iter()
        .find(|(name, _)| *name == period)
        .map(|(_, seconds)| *seconds)
        .unwrap_or_else(|| panic!("{period} is no aggregation period this service offers"))
}

/// One case about a window that had already CLOSED when the submission covering it was
/// posted: the aggregation period asked about, how many of that period's windows back the
/// submitted data sits, the media type it is posted under and how many submissions the
/// window holds.
struct ClosedWindow {
    label: &'static str,
    /// The modelled `AggregationPeriod` the request names as `period`, and nothing else —
    /// which is what asks for the group's latest aggregated profile of that length.
    period: &'static str,
    /// How many windows of that period before the one the clock is in the data sits in: 1
    /// is the LAST window to have closed, which is the window an agent that has just
    /// finished collecting submits for.
    windows_back: i64,
    content_type: &'static str,
    /// How many submissions the window holds, each with its own `profileToken` and its own
    /// frame.
    posts: usize,
}

impl ClosedWindow {
    /// The start of the window this case's data covers, in epoch seconds, worked out from
    /// the clock and the period's length alone — so the expectation is not the handler's
    /// own arithmetic read back.
    fn window_start(&self, now: i64) -> i64 {
        let length = period_length(self.period);
        now.div_euclid(length)
            .saturating_sub(self.windows_back)
            .saturating_mul(length)
    }

    /// The bytes submission `post` of this case is posted as, in the representation it is
    /// posted under: a profile stating the start of the period its data covers and one
    /// frame of its own, so which submissions an answer holds is readable off the answer.
    ///
    /// Each submission states one millisecond more than the last, which keeps the bytes of
    /// a window's submissions distinct without moving any of them out of the window.
    fn posted_bytes(&self, window_start: i64, post: usize) -> Vec<u8> {
        const MILLIS_PER_SECOND: i64 = 1_000;
        let counted = i64::try_from(post).unwrap_or(0);
        let start = window_start.saturating_mul(MILLIS_PER_SECOND) + counted;
        let frame = format!("F{post}");
        if self.content_type == ION {
            ion_rs::Element::from(ion_rs::Struct::from_iter([
                (
                    ion_rs::Symbol::from("start"),
                    ion_rs::Element::from(start),
                ),
                (
                    ion_rs::Symbol::from("callgraph"),
                    ion_rs::Element::from(ion_rs::Struct::from_iter([(
                        ion_rs::Symbol::from(frame.as_str()),
                        ion_rs::Element::from(counted + 1),
                    )])),
                ),
            ]))
            .encode_as(ion_rs::v1_0::Binary)
            .expect("an encodable Ion document")
        } else {
            serde_json::to_vec(&serde_json::json!({
                "start": start,
                "callgraph": { frame: counted + 1 },
            }))
            .expect("a serializable document")
        }
    }

    /// The profiling DATA the aggregated profile of this case's window holds: the frame
    /// each of its submissions contributed. What the document states about the AGGREGATION
    /// itself — the window it covers, the agents that reported into it — is the service's
    /// own and is asserted on its own ([`aggregation_of`]).
    fn expected_data(&self) -> ion_rs::Element {
        let mut callgraph = serde_json::Map::new();
        for post in 0..self.posts {
            callgraph.insert(
                format!("F{post}"),
                serde_json::json!(i64::try_from(post).unwrap_or(0) + 1),
            );
        }
        data_of(&element_of(
            &serde_json::to_vec(&serde_json::json!({ "callgraph": callgraph }))
                .expect("a serializable document"),
        ))
    }
}

/// The table: rows differ in the period the window belongs to (every one the model
/// declares), how far back that window closed, the representation the data was posted in,
/// and how many agents reported into the window.
fn closed_window_rows() -> Vec<ClosedWindow> {
    vec![
        ClosedWindow {
            label: "the last closed five-minute window",
            period: "PT5M",
            windows_back: 1,
            content_type: JSON,
            posts: 1,
        },
        ClosedWindow {
            label: "the last closed five-minute window, posted as Ion",
            period: "PT5M",
            windows_back: 1,
            content_type: ION,
            posts: 1,
        },
        ClosedWindow {
            label: "two agents reporting into the last closed five-minute window",
            period: "PT5M",
            windows_back: 1,
            content_type: JSON,
            posts: 2,
        },
        ClosedWindow {
            label: "a five-minute window twelve windows back",
            period: "PT5M",
            windows_back: 12,
            content_type: JSON,
            posts: 1,
        },
        ClosedWindow {
            label: "the last closed hour",
            period: "PT1H",
            windows_back: 1,
            content_type: JSON,
            posts: 1,
        },
        ClosedWindow {
            label: "the hour before the last closed one, two agents, posted as Ion",
            period: "PT1H",
            windows_back: 2,
            content_type: ION,
            posts: 2,
        },
        ClosedWindow {
            label: "the last closed day",
            period: "P1D",
            windows_back: 1,
            content_type: JSON,
            posts: 1,
        },
        ClosedWindow {
            label: "the day before the last closed one, three agents",
            period: "P1D",
            windows_back: 2,
            content_type: JSON,
            posts: 3,
        },
    ]
}

/// A submission whose data covers a window that has ALREADY closed is still not
/// retrievable at the instant it is accepted — the pass that collects it has not run yet —
/// and is owed its aggregated profile once that pass is through: the `GetProfile` naming
/// that period then answers 200 with the aggregated document of THAT window, the one the
/// submission's data sits in.
///
/// The clock is read clear of every window boundary, so each row's window closed well
/// before the submission was posted, and the only thing standing between the post and the
/// 200 is the pass over the submission itself.
#[tokio::test]
async fn a_submission_covering_a_closed_window_is_retrievable_once_the_pass_has_run() {
    let harness = Harness::new();
    let now = now_clear_of_window_boundaries().await;

    for (index, row) in closed_window_rows().iter().enumerate() {
        let name = format!("closed-{index}");
        let case = row.label;
        assert_eq!(
            create_group(&harness, &name).await.status,
            201,
            "case {case:?}: create must succeed"
        );

        let window_start = row.window_start(now);
        let length = period_length(row.period);
        for post in 0..row.posts {
            let answer = harness
                .post_agent_profile(
                    &name,
                    row.content_type,
                    Some(&format!("ptok-closed-{index}-{post}")),
                    row.posted_bytes(window_start, post),
                )
                .await;
            assert_eq!(
                answer.status,
                204,
                "case {case:?}: post {post} must answer 204, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
        }

        let query = format!("?period={}", row.period);
        // The window closed long ago, and the submission covering it has only just
        // arrived: an accepted submission is not an aggregated profile.
        let submitted = harness
            .get_profile_accepting(&name, &query, Some(JSON))
            .await;
        assert_eq!(
            submitted.status,
            404,
            "case {case:?}: a submission the aggregation pass has not run over yet must not \
             be served, got {} {}",
            submitted.status,
            String::from_utf8_lossy(&submitted.body)
        );
        assert_eq!(
            submitted.error_message(),
            NO_PROFILES_IN_TIME_RANGE,
            "case {case:?}: expected the fixed no-data text, got {:?}",
            submitted.error_message()
        );

        harness.complete_aggregation_pass(&name);
        let answer = harness
            .get_profile_accepting(&name, &query, Some(JSON))
            .await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: the aggregated profile of a window that closed before the post \
             must be served once the pass over the submission has run, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.content_type.as_deref(),
            Some(JSON),
            "case {case:?}: expected the accepted representation to be announced, got {:?}",
            answer.content_type
        );
        assert_eq!(
            profile_data(&answer.body),
            row.expected_data(),
            "case {case:?}: expected the window's submitted frames, got {}",
            element_of(&answer.body)
        );

        // The document states the window it covers — the aligned window the data sits in,
        // not any instant the uploads stated — and how many agents reported into it.
        let (start, end, reported_agents) = aggregation_of(case, &answer.body);
        assert_eq!(
            start,
            iso_millis(window_start),
            "case {case:?}: expected the window's start, got {}",
            element_of(&answer.body)
        );
        assert_eq!(
            end,
            iso_millis(window_start + length),
            "case {case:?}: expected the window's end, got {}",
            element_of(&answer.body)
        );
        assert_eq!(
            reported_agents,
            i64::try_from(row.posts).unwrap_or(0),
            "case {case:?}: expected the window's {} submissions to be reported, got {}",
            row.posts,
            element_of(&answer.body)
        );
    }
}

/// How long the poll below waits for a closed window's aggregated profile before failing
/// the case.
///
/// The service's pass over a submission takes a few seconds
/// (`state::AGGREGATION_PASS_MILLIS`), so the answer arrives well inside this; the
/// deadline is deliberately generous so a window whose aggregate never arrives fails here
/// instead of spinning, which is the defect this suite's poll is about.
const AGGREGATION_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

/// How often that poll looks again.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// The whole agent round trip a client written against the real service drives:
/// `ConfigureAgent`, `PostAgentProfile` for the window that has just closed,
/// `DescribeProfilingGroup`, then `GetProfile` polled until the aggregated profile is
/// there — tolerating only the modelled 404 `ResourceNotFoundException`, the way such a
/// client does.
///
/// The poll ENDS: every attempt answers either the 200 payload or that one modelled
/// refusal, and the payload arrives inside the deadline, so a caller never waits on a
/// window the service will not aggregate.
#[tokio::test]
async fn polling_get_profile_after_a_post_for_a_closed_window_completes() {
    let harness = Harness::new();
    let name = "closed-round-trip";
    assert_eq!(
        create_group(&harness, name).await.status,
        201,
        "create must succeed"
    );
    assert_eq!(
        harness.configure_agent(name).await.status,
        200,
        "configuring the agent must succeed"
    );

    let period = "PT5M";
    let length = period_length(period);
    let row = ClosedWindow {
        label: "the agent round trip",
        period,
        windows_back: 1,
        content_type: JSON,
        posts: 1,
    };
    let window_start = row.window_start(now_clear_of_window_boundaries().await);
    assert_eq!(
        harness
            .post_agent_profile(
                name,
                JSON,
                Some("ptok-round-trip"),
                row.posted_bytes(window_start, 0),
            )
            .await
            .status,
        204,
        "the post must succeed"
    );
    assert!(
        harness
            .describe(name)
            .await
            .json()
            .pointer("/profilingStatus/latestAgentProfileReportedAt")
            .is_some(),
        "the post must be recorded as received"
    );

    let query = format!("?period={period}");
    let deadline = std::time::Instant::now() + AGGREGATION_DEADLINE;
    let mut attempts = 0usize;
    let served = loop {
        let answer = harness
            .get_profile_accepting(name, &query, Some(JSON))
            .await;
        if answer.status == 200 {
            break answer;
        }
        // A client polling for an aggregation tolerates exactly one answer besides the
        // payload, so anything else — another status, another code, no answer at all —
        // ends the poll here.
        assert_eq!(
            answer.status,
            404,
            "attempt {attempts}: GetProfile must answer the payload or its modelled 404, \
             got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "attempt {attempts}: expected ResourceNotFoundException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_message(),
            NO_PROFILES_IN_TIME_RANGE,
            "attempt {attempts}: expected the fixed no-data text, got {:?}",
            answer.error_message()
        );
        assert!(
            std::time::Instant::now() < deadline,
            "the aggregated profile of a closed window never became retrievable: \
             {attempts} attempts answered 404"
        );
        attempts += 1;
        tokio::time::sleep(POLL_INTERVAL).await;
    };

    let case = row.label;
    let (start, end, reported_agents) = aggregation_of(case, &served.body);
    assert_eq!(
        start,
        iso_millis(window_start),
        "expected the window's start, got {}",
        element_of(&served.body)
    );
    assert_eq!(
        end,
        iso_millis(window_start + length),
        "expected the window's end, got {}",
        element_of(&served.body)
    );
    assert_eq!(
        reported_agents, 1,
        "expected the one submission to be reported, got {}",
        element_of(&served.body)
    );
    assert_eq!(
        profile_data(&served.body),
        row.expected_data(),
        "expected the submitted frame, got {}",
        element_of(&served.body)
    );
}

/// An ACCEPTED submission is not an aggregated profile: whatever window its data covers,
/// and however long ago that window closed, the pass that collects it into the window's
/// aggregated profile runs after the submission arrives — so the request that names the
/// window answers the modelled 404 with the fixed no-data text while the group already
/// records the submission as RECEIVED, and answers 200 only once that pass is through.
///
/// Nothing writes to the store between the two: each row's submission is posted over HTTP
/// and the CLOCK alone moves it from pending to retrievable, which the poll waits for.
/// Rows differ in the aggregation period asked about (every one the model declares), how
/// many of that period's windows ago the data's window closed, the representation the data
/// was posted in and how many agents reported into the window
/// ([`closed_window_rows`]) — the window a submission covers is what the rule
/// generalises over, and in every row it had closed before the post.
#[tokio::test]
async fn an_accepted_submission_is_pending_the_pass_whatever_window_it_covers() {
    let harness = Harness::new();
    let now = now_clear_of_window_boundaries().await;
    let rows = closed_window_rows();

    // Each row is posted and then read straight away, so the read that must find the
    // submission pending is the one right after its own post; the passes then run
    // alongside each other while the rows are polled below.
    for (index, row) in rows.iter().enumerate() {
        let name = format!("pass-{index}");
        let case = row.label;
        assert_eq!(
            create_group(&harness, &name).await.status,
            201,
            "case {case:?}: create must succeed"
        );
        let window_start = row.window_start(now);
        for post in 0..row.posts {
            let answer = harness
                .post_agent_profile(
                    &name,
                    row.content_type,
                    Some(&format!("ptok-pass-{index}-{post}")),
                    row.posted_bytes(window_start, post),
                )
                .await;
            assert_eq!(
                answer.status,
                204,
                "case {case:?}: post {post} must answer 204, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
        }

        // Received — the group says so — and not yet aggregated.
        let described = harness.describe(&name).await;
        assert!(
            described
                .json()
                .pointer("/profilingStatus/latestAgentProfileReportedAt")
                .is_some(),
            "case {case:?}: the submission must be recorded as received, got {}",
            String::from_utf8_lossy(&described.body)
        );

        let query = format!("?period={}", row.period);
        let pending = harness
            .get_profile_accepting(&name, &query, Some(JSON))
            .await;
        assert_eq!(
            pending.status,
            404,
            "case {case:?}: a submission the aggregation pass has not run over yet must not \
             be served, got {} {}",
            pending.status,
            String::from_utf8_lossy(&pending.body)
        );
        assert_eq!(
            pending.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            String::from_utf8_lossy(&pending.body)
        );
        assert_eq!(
            pending.error_message(),
            NO_PROFILES_IN_TIME_RANGE,
            "case {case:?}: expected the fixed no-data text, got {:?}",
            pending.error_message()
        );
        let times = list_profile_times(&harness, &name, row.period).await;
        assert_eq!(
            reported_times(&times),
            0,
            "case {case:?}: a submission still pending the pass must contribute no profile \
             time, got {}",
            String::from_utf8_lossy(&times.body)
        );
    }

    // And then the clock alone: each row's aggregated profile becomes retrievable, stating
    // the window the submission's data covers and the agents that reported into it.
    for (index, row) in rows.iter().enumerate() {
        let name = format!("pass-{index}");
        let case = row.label;
        let window_start = row.window_start(now);
        let length = period_length(row.period);
        let query = format!("?period={}", row.period);
        let deadline = std::time::Instant::now() + AGGREGATION_DEADLINE;
        let mut attempts = 0usize;
        let served = loop {
            let answer = harness
                .get_profile_accepting(&name, &query, Some(JSON))
                .await;
            if answer.status == 200 {
                break answer;
            }
            assert_eq!(
                answer.status,
                404,
                "case {case:?}, attempt {attempts}: GetProfile must answer the payload or \
                 its modelled 404, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_message(),
                NO_PROFILES_IN_TIME_RANGE,
                "case {case:?}, attempt {attempts}: expected the fixed no-data text, got {:?}",
                answer.error_message()
            );
            assert!(
                std::time::Instant::now() < deadline,
                "case {case:?}: the aggregation pass never let the submission through: \
                 {attempts} attempts answered 404"
            );
            attempts += 1;
            tokio::time::sleep(POLL_INTERVAL).await;
        };

        let (start, end, reported_agents) = aggregation_of(case, &served.body);
        assert_eq!(
            start,
            iso_millis(window_start),
            "case {case:?}: expected the window's start, got {}",
            element_of(&served.body)
        );
        assert_eq!(
            end,
            iso_millis(window_start + length),
            "case {case:?}: expected the window's end, got {}",
            element_of(&served.body)
        );
        assert_eq!(
            reported_agents,
            i64::try_from(row.posts).unwrap_or(0),
            "case {case:?}: expected the window's {} submissions to be reported, got {}",
            row.posts,
            element_of(&served.body)
        );
        assert_eq!(
            profile_data(&served.body),
            row.expected_data(),
            "case {case:?}: expected the window's submitted frames, got {}",
            element_of(&served.body)
        );
        let times = list_profile_times(&harness, &name, row.period).await;
        assert!(
            reported_times(&times) >= 1,
            "case {case:?}: the window whose pass has run must be reported as a profile \
             time, got {}",
            String::from_utf8_lossy(&times.body)
        );
    }
}
