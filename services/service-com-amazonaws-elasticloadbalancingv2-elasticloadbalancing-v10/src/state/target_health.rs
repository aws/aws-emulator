// SPDX-License-Identifier: CC0-1.0

//! The TARGET HEALTH state machine of one target group: what `DescribeTargetHealth`
//! reports for a registration, and the two spans the clock owes it.
//!
//! A registration is not born serving traffic and does not stop serving it the instant a
//! caller asks: `TargetHealthStateEnum`
//! (`smithy_query(query="shape",shape="TargetHealthStateEnum")`) names
//! `initial`/`healthy`/`unhealthy`/`unused`/`draining`/`unavailable`, and
//! `TargetHealth$State`'s own documentation gives each of them a reason
//! (`TargetHealth$Reason`, the `TargetHealthReasonEnum` values quoted below). Two of
//! those states are TRANSIENT — the clock, not a caller, is what ends them — and this
//! module is the one place both are decided, so the read that REPORTS a target's health
//! and the data plane that decides whether a target may be sent a request cannot drift:
//!
//! * `initial` — "the load balancer is still sending the target the minimum number of
//!   health checks required to determine its health status" — which begins when health
//!   checking on the registration begins: when it is registered, or, for a target group
//!   no load balancer forwarded to yet, when the group first becomes one a load balancer
//!   forwards to ([`TargetGroupRecord::begin_health_checking`]). A group nothing forwards
//!   to has nothing checking its targets at all, which is the `unused` state below.
//! * `draining` — "the load balancer is draining connections to the target", the state
//!   `DeregisterTargets` puts a registration in: it stops receiving NEW requests
//!   immediately and leaves the target group only once the deregistration delay has
//!   passed ([`deregistration_delay`]).
//!
//! Past the registration span the state is the one the CHECKS reached, never a constant:
//! `unhealthy` once the group's `UnhealthyThresholdCount` consecutive checks failed, and the
//! second `initial` (`Elb.InitialHealthChecking`) until a check has FOUND this registration
//! healthy — while no check of it has been made at all, and while the checks made of it have
//! failed without reaching that threshold. `healthy` is a finding of the checks, so a
//! registration none of them has been made of is one whose health the load balancer has not
//! determined yet, and one whose checks are failing is never reported healthy on the way to its
//! verdict. The first check is owed at the instant checking begins and every later one comes one
//! `HealthCheckIntervalSeconds` after the last ([`health_check_interval`]) where a caller asked
//! for that cadence, and otherwise one span of this emulator's own transitions after it, so a
//! verdict is reached no sooner than the schedule the caller asked for could have decided it and
//! a group nobody configured a cadence for settles like every other clock-owed step of this
//! emulator — except that no run of failing checks holds its `unhealthy` verdict back longer
//! than [`VERDICT_WITHIN`], the span this emulator keeps a verdict observable in, which is what
//! a cadence and a threshold that would put the first verdict minutes out are spaced to fit
//! inside; what a READ reports is untouched either way, so that spacing is observable only in
//! the timing of the verdicts. WHY those checks failed
//! is reported with the verdict ([`CheckFailure`]): a check the target answered with a status
//! its group's `Matcher` does not accept is the model's own `Target.ResponseCodeMismatch`, one
//! that came back with an error or with an answer that is no response at all is its
//! `Target.FailedHealthChecks`, and one nothing answered within the check's own timeout is
//! worded as the family of load balancer performing it words it.
//! What one check observes is decided in [`super::host_counts`], against the effective
//! health-check port and the ports something behind the registration is listening on — the ones
//! its scheduler declared ([`ServedPorts`]), and for an instance of this account the ones Amazon
//! EC2 holds for it. A registration whose checks pass keeps the `healthy` they found it, and one
//! whose checks found NOTHING — the invocation a `lambda` group's check is, where this emulator
//! could not carry it — is still one whose health no check has determined: silence neither fails
//! a registration nor finds it healthy.
//!
//! Every span is stored as the instant it comes due, never slept for; a read
//! materialises what the clock already owes ([`TargetGroupRecord::settle_draining`]) and
//! writes nothing else.
//!
//! A description carries two MEMBERS BESIDE that health, and this module is where both are
//! decided as well, so every reader of a registration reports them alike: the administrative
//! override in effect on the target ([`administrative_override`]), which every description
//! carries, and the anomaly detection of it ([`anomaly_detection`]), which is one of the
//! optional blocks a request's `Include` options select ([`IncludedBlocks`]).

use std::ops::RangeInclusive;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AdministrativeOverride, AnomalyDetection, TargetDescription, TargetHealth,
    TargetHealthDescription,
};

use super::{
    target_group_attribute, targets_per_target_group, GuardingGroups, RegisteredTarget,
    TargetGroupRecord, TargetKey, LAMBDA_TARGET_TYPE, SETTLE_AFTER,
};

/// The target group attribute that holds how long a deregistering target drains, from
/// `TargetGroupAttribute$Key`'s own catalog: "`deregistration_delay.timeout_seconds` -
/// The amount of time, in seconds, for Elastic Load Balancing to wait before changing
/// the state of a deregistering target from `draining` to `unused`."
pub const DEREGISTRATION_DELAY_ATTRIBUTE: &str = "deregistration_delay.timeout_seconds";

/// How long a deregistering registration of `group` drains before it leaves the group:
/// the value [`DEREGISTRATION_DELAY_ATTRIBUTE`] carries for it — what a caller wrote,
/// else the documented default the attribute is carried at.
///
/// A DURATION the caller configures, so it keeps the value the attribute reports rather
/// than being shortened to the span this emulator chooses for a transition of its own:
/// the whole point of the attribute is that a client can ask for the drain it needs and
/// then observe exactly it. A target group the attribute does not apply to — "if the
/// target is a Lambda function, this attribute is not supported" — drains for no time at
/// all, so such a deregistration takes effect at once.
///
/// BOUNDED by the attribute's own documented range
/// (`super::AttributeDomain::integer_ceiling`), which is the range a write of it is refused
/// outside: the span is arithmetic on the clock, so it is held to what the collection
/// could legitimately carry rather than to whatever a number parses to. A domain stating
/// no range bounds nothing and the configured value stands as it is — the arithmetic on it
/// is checked either way ([`due_at`]).
pub fn deregistration_delay(group: &TargetGroupRecord) -> Duration {
    let Some(spec) = target_group_attribute(DEREGISTRATION_DELAY_ATTRIBUTE) else {
        return Duration::ZERO;
    };
    if !spec.applies_to(group) {
        return Duration::ZERO;
    }
    let ceiling = spec
        .domain
        .integer_ceiling()
        .and_then(|ceiling| u64::try_from(ceiling).ok());
    group
        .attributes
        .get(DEREGISTRATION_DELAY_ATTRIBUTE)
        .cloned()
        .or_else(|| spec.default_for(group))
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map_or(Duration::ZERO, |seconds| {
            Duration::from_secs(ceiling.map_or(seconds, |ceiling| seconds.min(ceiling)))
        })
}

/// The span between two health checks of one registration of `group`: the cadence the CALLER
/// asked this group's checks to run at — its `HealthCheckIntervalSeconds`, "the approximate
/// amount of time, in seconds, between health checks of an individual target" — else the span
/// this emulator owes a transition of its own ([`SETTLE_AFTER`]), and in neither case longer
/// than the spacing that keeps the VERDICT those checks decide observable
/// ([`verdict_spacing`]).
///
/// A cadence a request NAMED is a duration the caller configured, so it keeps the value the
/// target group reports for it rather than being shortened to the span this emulator chooses
/// for a transition of its own: the point of the member is that a client can ask for the cadence
/// it needs and then observe exactly it, and the cadence is what decides when a verdict is in —
/// `UnhealthyThresholdCount` consecutive failing checks cannot be before that many intervals
/// have passed. It keeps that value for as long as the verdict it decides stays observable,
/// which is the second bound below.
///
/// A cadence NOBODY asked for is a different thing: it is the value `CreateTargetGroup` settled
/// from the member's documented default, so the wait between two checks of such a group is a
/// span this emulator chooses, and it chooses the one it gives every other transition its own
/// clock owes ([`SETTLE_AFTER`], the span this crate settles a load balancer's provisioning, a
/// registration's own and a gone host's drain in — CHOSEN, with no documented duration behind
/// it). The verdict is still a number of CHECKS away — the two thresholds are counts, not
/// durations, and they stand as the group reports them — so a group left at its defaults
/// reaches `unhealthy` after as many failing checks as `UnhealthyThresholdCount` asks for and
/// returns to `healthy` after as many passing ones, in the span this emulator settles a
/// transition in rather than the minutes the documented cadence would take. What a READ reports
/// is untouched: `HealthCheckIntervalSeconds` answers the documented default for such a group,
/// because that is the value AWS carries it at.
///
/// BOUNDED by the member's own documented range ("the range is 5-300"), which is the range a
/// write of it is refused outside: the span is arithmetic on the clock, so a configured cadence
/// is held to what a target group could legitimately carry rather than to whatever a stored
/// number says.
///
/// And BOUNDED, second, by how far away the verdict those checks decide may be: an `unhealthy`
/// verdict is a THRESHOLD of consecutive failing checks at this spacing, so a long cadence and a
/// high threshold put the first verdict about a registration minutes out
/// ([`verdict_spacing`]). A group whose own run already fits inside that span is spaced by
/// exactly the cadence it asked for; one whose run does not is spaced by what fits it in — and
/// the return to `healthy` its passing checks then decide comes at the same spacing. The counts
/// stay counts either way — as many checks are made, in the same order, each judged by what it
/// found — and what a read reports is untouched.
pub fn health_check_interval(group: &TargetGroupRecord) -> Duration {
    let asked_for = if group.health_check.interval_configured {
        let configured = group.health_check.interval_seconds.clamp(
            MIN_HEALTH_CHECK_INTERVAL_SECONDS,
            MAX_HEALTH_CHECK_INTERVAL_SECONDS,
        );
        Duration::from_secs(configured.unsigned_abs().into())
    } else {
        SETTLE_AFTER
    };
    asked_for.min(verdict_spacing(group))
}

/// The longest two checks of `group` may be apart if the run of consecutive FAILING checks that
/// decides an `unhealthy` verdict about one of its registrations is to fit inside
/// [`VERDICT_WITHIN`]: that span over the number of checks the run takes.
///
/// The run is the group's `UnhealthyThresholdCount` — "the number of consecutive health check
/// failures required before considering a target unhealthy" — which is the count that holds a
/// registration in `initial` while nothing answers it, and so the one that decides how far away
/// the verdict a client is waiting for is. It is read exactly as the probe counting those checks
/// reads it ([`TargetProbe::counted`], where a threshold under one still takes one check), so the
/// span this yields bounds the run that probe really requires whatever number the group carries.
fn verdict_spacing(group: &TargetGroupRecord) -> Duration {
    let checks = group.health_check.unhealthy_threshold.max(1).unsigned_abs();
    VERDICT_WITHIN / checks
}

/// How long the run of consecutive failing checks that decides an `unhealthy` verdict may hold
/// that verdict back.
///
/// The emulator's OWN span, not an AWS one: nothing AWS publishes bounds how far away a verdict
/// is, and the two members that decide it reach minutes at the top of their ranges — a target
/// group at the longest cadence and the most consecutive failures the threshold admits would
/// leave every registration of it `initial` for the better part of an hour. A client asks
/// `DescribeTargetHealth` whether its target is reachable and waits for the answer, so a verdict
/// that far out is one no caller of this emulator ever observes; the series of host counts the
/// same checks feed is published on a span of this emulator's own for exactly that reason
/// ([`super::host_counts`]).
///
/// Long enough that a group asking for a prompt verdict is spaced by precisely the cadence it
/// asked for — the shortest cadence `HealthCheckIntervalSeconds` admits at the most consecutive
/// failures the threshold admits, and a ten-second cadence at the fewest — and short enough that
/// the slowest pair the two members admit still reaches its verdict inside one of these spans.
/// It bounds the SPACING of the checks alone: which verdict they reach, and how many of them it
/// takes, are untouched, and so is every value a read of the group reports — including
/// `HealthCheckIntervalSeconds` itself, which answers the cadence the write stored even where
/// this spaces the checks closer together. The divergence is therefore observable only in the
/// TIMING of the verdicts, never in a read.
///
/// The run it is read against is the FAILING one, which is the run that holds a verdict back from
/// a client: a registration nothing answers stays `initial` for the whole of it, while the return
/// to `healthy` follows at the same spacing once the checks pass again —
/// `HealthyThresholdCount` of them. Reading it against the larger of the two thresholds instead
/// would shorten the spacing BELOW the cadence a caller asked for even for a group whose failing
/// run already fits, which is the cadence a client configured in order to observe it (and which
/// `tests/all/host_count_health_checks.rs` pins as the soonest a verdict may arrive).
const VERDICT_WITHIN: Duration = Duration::from_secs(20);

/// The shortest cadence `HealthCheckIntervalSeconds` may be written at ("the range is
/// 5-300").
const MIN_HEALTH_CHECK_INTERVAL_SECONDS: i32 = 5;

/// The longest cadence that member may be written at; see
/// [`MIN_HEALTH_CHECK_INTERVAL_SECONDS`].
const MAX_HEALTH_CHECK_INTERVAL_SECONDS: i32 = 300;

/// How long one health check of a registration of `group` waits for an answer: the group's own
/// `HealthCheckTimeoutSeconds` — "the amount of time, in seconds, during which no response from
/// a target means a failed health check".
///
/// A DURATION the caller configures, so it keeps the value the target group reports for it, and
/// BOUNDED by the member's own documented range ("the range is 2–120 seconds") for the same
/// reason the cadence above is: the span is arithmetic on the clock, so it is held to what a
/// target group could legitimately carry. A group carrying no timeout at all waits the shortest
/// the member allows.
///
/// Read where a check WAITS for an answer — the invocation a check of a `lambda` target group is
/// ([`crate::service::lambda_health_check`]) — so the wait and the value a client reads for the
/// member are the same span.
pub fn health_check_timeout(group: &TargetGroupRecord) -> Duration {
    let configured = group.health_check.timeout_seconds.clamp(
        MIN_HEALTH_CHECK_TIMEOUT_SECONDS,
        MAX_HEALTH_CHECK_TIMEOUT_SECONDS,
    );
    Duration::from_secs(configured.unsigned_abs().into())
}

/// The shortest wait `HealthCheckTimeoutSeconds` may be written at ("the range is 2–120
/// seconds").
const MIN_HEALTH_CHECK_TIMEOUT_SECONDS: i32 = 2;

/// The longest wait that member may be written at; see [`MIN_HEALTH_CHECK_TIMEOUT_SECONDS`].
const MAX_HEALTH_CHECK_TIMEOUT_SECONDS: i32 = 120;

/// The instant a span that starts `now` comes due.
///
/// The addition is CHECKED: a span the clock cannot represent an end for is due at once,
/// so no arithmetic on a caller-configured duration can end a request in a panic.
pub(super) fn due_at(now: SystemTime, span: Duration) -> SystemTime {
    now.checked_add(span).unwrap_or(now)
}

/// The `TargetHealthStateEnum` values this machine reports, and the
/// `TargetHealthReasonEnum` reason and description each is reported with — the model's
/// own words for the condition, so one state cannot be worded two ways.
const INITIAL: (&str, &str, &str) = (
    INITIAL_STATE,
    "Elb.RegistrationInProgress",
    "Target registration is in progress",
);
const DRAINING: (&str, &str, &str) = (
    "draining",
    "Target.DeregistrationInProgress",
    "Target deregistration is in progress",
);
const UNUSED_NOT_IN_USE: (&str, &str, &str) = (
    "unused",
    "Target.NotInUse",
    "Target group is not configured to receive traffic from the load balancer",
);
const UNUSED_NOT_REGISTERED: (&str, &str, &str) = (
    "unused",
    "Target.NotRegistered",
    "Target is not registered to the target group",
);
const UNAVAILABLE: (&str, &str, &str) = (
    "unavailable",
    "Target.HealthCheckDisabled",
    "Health checks are not enabled for this target group",
);
const UNHEALTHY_TIMED_OUT: (&str, &str, &str) =
    (UNHEALTHY_STATE, "Target.Timeout", "Request timed out");
/// The `unhealthy` a load balancer that reads no response out of the check itself reports:
/// "`Target.FailedHealthChecks` - The load balancer received an error while establishing a
/// connection to the target or the target response was malformed" (`TargetHealth$Reason`).
///
/// Which failure a family of load balancer words a check nothing answered as is the
/// family's own: an Application Load Balancer waits out the check's timeout and reports
/// [`UNHEALTHY_TIMED_OUT`], while a Network or Gateway Load Balancer hands the connection on
/// and reports the failed checks ([`unhealthy_for`]). A check that came back with an ERROR or
/// with an answer that is no response is this reason for EVERY family
/// ([`CheckFailure::Malformed`]): it is the condition the model gives this reason for, and
/// not one family's wording of a check that went unanswered.
const UNHEALTHY_FAILED_CHECKS: (&str, &str, &str) = (
    UNHEALTHY_STATE,
    "Target.FailedHealthChecks",
    "Health checks failed",
);
/// The other `initial` the model names, for a registration whose checks have STARTED and
/// have not decided yet: "`Elb.InitialHealthChecking` - The load balancer is still sending
/// the target the minimum number of health checks required to determine its health status"
/// (`TargetHealth$Reason`).
const INITIAL_HEALTH_CHECKING: (&str, &str, &str) = (
    INITIAL_STATE,
    "Elb.InitialHealthChecking",
    "Health checks in progress",
);

/// The `TargetHealthReasonEnum` value an `unhealthy` registration whose checks the target
/// ANSWERED — with a status its group's `Matcher` does not count as a success — is reported
/// under: "`Target.ResponseCodeMismatch` - The health checks did not return an expected HTTP
/// code" (`TargetHealth$Reason`).
///
/// A different fact from the reasons beside it: those are a check nothing answered at all,
/// while this one is a check that WAS answered and whose answer failed the group's own success
/// criterion (`super::matcher_admits`).
const RESPONSE_CODE_MISMATCH_REASON: &str = "Target.ResponseCodeMismatch";

/// How that reason is DESCRIBED: the codes the checks were answered with, which is what the
/// load balancer observed about the mismatch — the status the target really answered, never a
/// constant.
fn response_code_mismatch_description(status: u16) -> String {
    format!("Health checks failed with these codes: [{status}]")
}

/// What one health check of a registration FOUND, as the load balancer performing it reports
/// the finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observed {
    /// The check succeeded: something answered it, and its answer is one the target group's
    /// own success criterion accepts.
    Passed,
    /// The check did not succeed, for the condition that held.
    Failed(CheckFailure),
}

/// WHY one health check did not succeed, which is what decides the `TargetHealth$Reason` a
/// registration whose verdict those checks reached is reported with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckFailure {
    /// NOTHING answered the check — the condition the family of load balancer performing it
    /// words as its own ([`unhealthy_for`]).
    Unanswered,
    /// The target ANSWERED, with the status carried here, and that status is not one its
    /// group's `Matcher` counts as a success ([`RESPONSE_CODE_MISMATCH_REASON`]).
    ResponseCode(u16),
    /// The check produced an ERROR instead of a response, or an answer that is no response at
    /// all — the condition the model words as "the load balancer received an error while
    /// establishing a connection to the target or the target response was malformed"
    /// ([`UNHEALTHY_FAILED_CHECKS`]).
    ///
    /// A different fact from both of the failures beside it: the check was not left without an
    /// answer within its own timeout, and nothing came back carrying a status to judge against
    /// the group's criterion, so there is no HTTP code to mismatch. Worded the same way
    /// whatever family of load balancer performed the check, because it is the model's own
    /// reason for that condition rather than one family's wording of an unanswered check.
    Malformed,
}

impl Observed {
    /// Whether the check succeeded, which is what a verdict is counted from.
    fn passed(self) -> bool {
        matches!(self, Observed::Passed)
    }

    /// The condition a check that did not succeed found, and `None` for one that did.
    fn failure(self) -> Option<CheckFailure> {
        match self {
            Observed::Passed => None,
            Observed::Failed(failure) => Some(failure),
        }
    }
}

/// How the family `family` of load balancer words an `unhealthy` verdict its checks reached.
///
/// The verdict itself is the same fact for every family — the group's
/// `UnhealthyThresholdCount` consecutive checks found nothing answering — and the reason is
/// how the load balancer performing them reports it: an Application Load Balancer sends an
/// HTTP request and waits out `HealthCheckTimeoutSeconds` for a response, which is the
/// `Target.Timeout` its own reason table words; a Network or Gateway Load Balancer reports
/// `Target.FailedHealthChecks` instead, the reason code the Network Load Balancer's target
/// health documentation gives a target whose checks did not succeed.
fn unhealthy_for(family: super::LoadBalancerFamily) -> (&'static str, &'static str, &'static str) {
    match family {
        super::LoadBalancerFamily::Application => UNHEALTHY_TIMED_OUT,
        super::LoadBalancerFamily::Network | super::LoadBalancerFamily::Gateway => {
            UNHEALTHY_FAILED_CHECKS
        }
    }
}

/// How an `unhealthy` verdict a family `family` of load balancer reached is REPORTED, given
/// what its last failing check found.
///
/// A check the target ANSWERED is reported as the mismatch it is — the model's own reason for
/// an answer the group's success criterion rejects, described by the status that answer carried
/// — one that came back with an error or an answer that is no response as the failed checks
/// that condition is the model's reason for ([`UNHEALTHY_FAILED_CHECKS`]), and a check nothing
/// answered as the family's own wording of that condition ([`unhealthy_for`]), which is also
/// what a verdict whose failures are no longer known is reported as.
fn unhealthy_report(
    family: super::LoadBalancerFamily,
    failure: Option<CheckFailure>,
) -> TargetHealth {
    match failure {
        Some(CheckFailure::ResponseCode(status)) => TargetHealth {
            state: Some(UNHEALTHY_STATE.to_string()),
            reason: Some(RESPONSE_CODE_MISMATCH_REASON.to_string()),
            description: Some(response_code_mismatch_description(status)),
        },
        Some(CheckFailure::Malformed) => reported(UNHEALTHY_FAILED_CHECKS),
        Some(CheckFailure::Unanswered) | None => reported(unhealthy_for(family)),
    }
}

/// The `TargetHealthStateEnum` value of a registration the load balancer can route to.
///
/// `TargetHealth$State`'s own documentation gives it no reason code ("a reason code is not
/// provided") and no description, so a healthy registration reports the state alone.
pub const HEALTHY_STATE: &str = "healthy";

/// The `TargetHealthStateEnum` value of a registration whose health checks the load balancer
/// could not get an answer to.
pub const UNHEALTHY_STATE: &str = "unhealthy";

/// The `TargetHealthStateEnum` value of a registration whose health the load balancer has NOT
/// determined yet: the one it reports both while the registration is coming up and while its
/// checks have begun without reaching a verdict ([`INITIAL`], [`INITIAL_HEALTH_CHECKING`]).
///
/// A registration in it is in neither host count, and a group holding one has no count to
/// report at all ([`TargetGroupRecord::reported_host_counts`]).
pub const INITIAL_STATE: &str = "initial";

/// Where the periodic health check of ONE registration has got to: what the checks have
/// observed in a row, and the verdict those runs have reached.
///
/// `UnhealthyThresholdCount` is "the number of consecutive health check failures required
/// before considering a target unhealthy" and `HealthyThresholdCount` "the number of
/// consecutive health check successes required before considering an unhealthy target
/// healthy", so the verdict only MOVES once that many consecutive checks disagree with it —
/// which is what [`TargetProbe::counted`] folds an observation into. Those checks are the ones
/// the group's own cadence makes due ([`health_check_interval`]), so a verdict is never
/// reached sooner than the threshold times that interval. A registration no check has
/// observed anything about carries the verdict it started with, which is the one the clock
/// owes it ([`TargetGroupRecord::health_of`]).
#[derive(Debug, Clone, Default)]
pub struct TargetProbe {
    /// Consecutive failing checks since the last passing one.
    failures: u32,
    /// Consecutive passing checks since the last failing one.
    passes: u32,
    /// What the runs have DECIDED: `Some(true)` once as many consecutive checks passed as
    /// the group's `HealthyThresholdCount`, `Some(false)` once as many failed as its
    /// `UnhealthyThresholdCount`, and `None` while neither threshold has been reached.
    verdict: Option<bool>,
    /// Whether a check has ever FOUND this registration passing, which is the only thing it is
    /// reported `healthy` on.
    ///
    /// `healthy` is a finding of the checks, never a state a registration carries before one
    /// found it: while nothing has found it passing and no verdict is in, the load balancer has
    /// not determined its health, which is the `initial` the model words as it "still sending
    /// the target the minimum number of health checks required to determine its health status"
    /// ([`Self::is_undetermined`]). So a registration no check of has been made, one whose
    /// checks are failing short of the threshold, and one whose checks could not be made at all
    /// are reported alike — as the registration whose health is still being determined.
    ///
    /// Kept across a later failing check, because a state the checks reached only moves once
    /// as many consecutive checks disagree with it as the group's own threshold takes
    /// ([`Self::counted`]): a registration the checks have found healthy is not reported as
    /// one whose health is still being determined on the way to an `unhealthy` verdict, and no
    /// read after a decided one gives the state back to `initial`.
    passed: bool,
    /// What the last FAILING check of this registration found, and `None` while none has
    /// failed: the condition an `unhealthy` verdict is reported with
    /// ([`unhealthy_report`]), which is a fact of the checks rather than of the caller.
    ///
    /// Kept across a passing check, so a verdict the failures reached is reported with the
    /// condition that reached it for as long as it stands; a later failing check replaces it,
    /// because the reason reported is the one the checks last found.
    failed_with: Option<CheckFailure>,
}

impl TargetProbe {
    /// Keep what one look at this registration found, WITHOUT counting it toward a verdict:
    /// what this emulator can see about a registration it looks at more often than the
    /// group's cadence runs a check.
    ///
    /// A verdict is a number of consecutive CHECKS away, and the checks are the ones the
    /// configured interval makes due ([`TargetProbe::counted`]), so a look taken between two
    /// of them moves no verdict and finds no health. What it keeps is the CONDITION a failing
    /// look found, which is what the verdict those checks reach is then reported with
    /// ([`Self::failed_with`]).
    pub fn saw(&mut self, observed: Observed) {
        if let Some(failure) = observed.failure() {
            self.failed_with = Some(failure);
        }
    }

    /// Fold one check's observation into the verdict: `observed` is what the check found, and
    /// the two thresholds are the target group's own.
    ///
    /// A threshold the group reports as less than one still takes one check, so a verdict is
    /// never reached by no check at all.
    pub fn counted(
        &mut self,
        observed: Observed,
        healthy_threshold: i32,
        unhealthy_threshold: i32,
    ) {
        self.saw(observed);
        let threshold = |configured: i32| configured.max(1).unsigned_abs();
        if observed.passed() {
            self.passed = true;
            self.failures = 0;
            self.passes = self.passes.saturating_add(1);
            if self.passes >= threshold(healthy_threshold) {
                self.verdict = Some(true);
            }
        } else {
            self.passes = 0;
            self.failures = self.failures.saturating_add(1);
            if self.failures >= threshold(unhealthy_threshold) {
                self.verdict = Some(false);
            }
        }
    }

    /// Whether the checks have decided this registration is unhealthy.
    pub fn is_unhealthy(&self) -> bool {
        self.verdict == Some(false)
    }

    /// Whether the load balancer has NOT determined this registration's health yet, which is
    /// the `initial` the model words as "the load balancer is still sending the target the
    /// minimum number of health checks required to determine its health status"
    /// (`TargetHealth$Reason`).
    ///
    /// Its health is undetermined while no verdict is in and NO check has found it passing:
    /// `healthy` is a finding of the checks, never a state a registration is defaulted to
    /// before one found it. So a registration no check of has been made, one whose checks have
    /// only failed short of the group's threshold, and one whose checks could not be made at
    /// all are reported alike — the load balancer is still sending the checks its health is
    /// determined from.
    ///
    /// A registration a check has found passing is not this, even where a later check failed:
    /// a state the checks reached moves only once as many consecutive checks disagree with it
    /// as the group's own threshold takes, which is what [`Self::counted`] folds — and a
    /// registration that already HAS a verdict is reported as that verdict for the same
    /// reason. So the state machine never gives a decided registration back to `initial`.
    pub fn is_undetermined(&self) -> bool {
        self.verdict.is_none() && !self.passed
    }

    /// What the last FAILING check of this registration found, and `None` while none has
    /// failed — the condition an `unhealthy` verdict is reported with.
    pub fn failed_with(&self) -> Option<CheckFailure> {
        self.failed_with
    }
}

/// The ports the workload behind a registration SERVES, as inclusive ranges of port
/// numbers — one `start..=start` per single port.
///
/// What a health check of that registration can get an answer on: a check CONNECTS to the
/// effective health-check port ([`TargetGroupRecord::health_check_port_of`]), so a port the
/// workload does not serve is a port nothing answers on, however healthy the workload
/// itself is. An EMPTY list declares nothing: the registration of a bare address is one this
/// service knows no ports for, and a check of it observes what it observes about the address
/// alone.
///
/// SHARED, not copied: one workload's declared ports are the same list for every
/// registration that stands for it, so registering it in several target groups costs one
/// allocation and cloning a registration costs none.
pub type ServedPorts = Arc<[RangeInclusive<i32>]>;

/// One reported health, from a `(state, reason, description)` triple above.
fn reported(state: (&str, &str, &str)) -> TargetHealth {
    TargetHealth {
        state: Some(state.0.to_string()),
        reason: Some(state.1.to_string()),
        description: Some(state.2.to_string()),
    }
}

/// The administrative override a target NOTHING overrides is under: the
/// `TargetAdministrativeOverrideStateEnum` value that says so, the
/// `TargetAdministrativeOverrideReasonEnum` reason code for it, and how Elastic Load
/// Balancing describes that state
/// (`smithy_query(query="shape",shape="TargetAdministrativeOverrideStateEnum")` and its
/// reason enum beside it).
const NO_OVERRIDE: (&str, &str, &str) = (
    "no_override",
    "AdministrativeOverride.NoOverride",
    "No override is currently active on target",
);

/// What `TargetHealthDescription$AdministrativeOverride` carries for one reported target:
/// "the administrative override information for the target".
///
/// The states the model names are `no_override` and the ones a ZONAL SHIFT puts a target in
/// (`TargetAdministrativeOverrideStateEnum`). A zonal shift is another service's resource —
/// started, updated and cancelled through Amazon Application Recovery Controller and held in
/// that service's own store, which this one cannot read — and no call of this service, and
/// nothing this store holds, puts an override in effect on a target of a target group. So
/// every target a health read reports here is one no override is active on, and the member
/// carries exactly the state the model names for that, never nothing: a client reading it to
/// decide whether a target is administratively drained has to be able to tell "no override"
/// from "not reported".
///
/// Answered for EVERY description, for a registered target and for one the group holds no
/// registration of alike: the member is the override in effect on the target the request
/// named, which is a fact about that target rather than about a registration.
pub fn administrative_override() -> AdministrativeOverride {
    AdministrativeOverride {
        state: Some(NO_OVERRIDE.0.to_string()),
        reason: Some(NO_OVERRIDE.1.to_string()),
        description: Some(NO_OVERRIDE.2.to_string()),
    }
}

/// The `AnomalyResultEnum` value a target no anomaly has been detected for is reported
/// under: "if no anomalies were detected, the result is `normal`"
/// (`TargetHealthDescription$AnomalyDetection`).
const NO_ANOMALY_DETECTED: &str = "normal";

/// The `MitigationInEffectEnum` value reported where no anomaly mitigation is in progress —
/// `AnomalyDetection$MitigationInEffect` "indicates whether anomaly mitigation is in
/// progress".
const MITIGATION_NOT_IN_EFFECT: &str = "no";

/// What `TargetHealthDescription$AnomalyDetection` carries for one reported target: "the
/// anomaly detection result for the target".
///
/// `Result` is "the latest anomaly detection result", and the model gives its baseline: "if
/// no anomalies were detected, the result is `normal`". Nothing in this emulator has found a
/// target's traffic anomalous — an anomaly is a deviation the load balancer detects in the
/// responses a target gives the traffic it is sent, which nothing here records a finding of —
/// so every target is reported at that baseline. `MitigationInEffect` is whether mitigation
/// "is in progress", which takes an anomaly to mitigate, so no target is under mitigation
/// either.
///
/// Reported only where the request's `Include` options select the block
/// ([`IncludedBlocks`]), which is the whole observable effect of that member.
pub fn anomaly_detection() -> AnomalyDetection {
    AnomalyDetection {
        result: Some(NO_ANOMALY_DETECTED.to_string()),
        mitigation_in_effect: Some(MITIGATION_NOT_IN_EFFECT.to_string()),
    }
}

/// The `DescribeTargetHealthInputIncludeEnum` option that selects the anomaly-detection
/// block of a description (`smithy_query(query="shape",
/// shape="DescribeTargetHealthInputIncludeEnum")`).
pub const INCLUDE_ANOMALY_DETECTION: &str = "AnomalyDetection";

/// The option of that same enum that selects EVERY optional block, so a caller that wants
/// them all names one value rather than enumerating them.
pub const INCLUDE_ALL: &str = "All";

/// Which OPTIONAL blocks of a target health description a read reports.
///
/// `DescribeTargetHealthInput$Include` is "used to include anomaly detection information":
/// the member selects blocks that are otherwise left out, so a block it does not name is
/// absent from the answer and one it names is present on every description the answer
/// carries. Read once, where the request is read, so the handler and the descriptions it
/// builds cannot disagree about what was asked for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IncludedBlocks {
    /// Whether the anomaly-detection block is part of the answer.
    pub anomaly_detection: bool,
}

impl IncludedBlocks {
    /// The blocks the `Include` options of a request select — none for a request that names
    /// no option at all, which is the answer without any optional block.
    ///
    /// The values are the enum's own ([`INCLUDE_ANOMALY_DETECTION`], [`INCLUDE_ALL`]); a
    /// value outside the enum never reaches here, because the member targets an enum and the
    /// request walk refuses one that is not among its values.
    pub fn selected_by(include: Option<&[String]>) -> Self {
        let options = include.unwrap_or_default();
        let selects = |option: &str| options.iter().any(|named| named == option);
        Self {
            anomaly_detection: selects(INCLUDE_ANOMALY_DETECTION) || selects(INCLUDE_ALL),
        }
    }

    /// The anomaly-detection block of one description under these options: the target's own
    /// anomaly detection where the options select it, and nothing where they do not.
    pub fn reported_anomaly_detection(self) -> Option<AnomalyDetection> {
        self.anomaly_detection.then(anomaly_detection)
    }
}

/// What another emulated service's SCHEDULER declares about one registration it makes in a
/// target group on behalf of a workload it runs ([`TargetGroupRecord::register_for`]).
///
/// Every member is the scheduler's own word about the workload, which is what this service
/// cannot know: where it is reached, and the two facts a health check of it is read against.
#[derive(Debug, Clone)]
pub struct SchedulerRegistration {
    /// The target's id: the address the load balancer reaches the workload at.
    pub id: String,
    /// The port it receives traffic on, and `None` for a registration that takes the target
    /// group's own port.
    pub port: Option<i32>,
    /// The Availability Zone the target is in, and `None` for one whose zone is unknown.
    pub availability_zone: Option<String>,
    /// The workload this registration stands for, which is what lets the scheduler that
    /// added it take it out again ([`TargetGroupRecord::scheduler_registrations`]).
    pub workload: String,
    /// The ports that workload LISTENS on ([`ServedPorts`]): a check of this registration
    /// gets an answer only on a port among them, so a health check the target group points
    /// at any other port is answered by nothing. A scheduler that declares none leaves the
    /// ports unknown.
    pub serves_ports: ServedPorts,
    /// The security groups GOVERNING that address ([`GuardingGroups`]): a check of this
    /// registration ARRIVES only if an inbound rule of one of them admits it from the load
    /// balancer performing it ([`super::health_check_firewall`]). A scheduler that declares
    /// none leaves the groups unknown, and the groups of the interface holding the address
    /// are read instead.
    pub guarded_by: GuardingGroups,
}

/// The only Availability Zone a target of a `lambda` target group is in
/// (`TargetDescription$AvailabilityZone`: "if the target type is `lambda`, this parameter is
/// optional and the only supported value is `all`").
///
/// A Lambda function is invoked rather than reached at an address, so it sits in no one zone
/// of the target group's network: it is reachable from the load balancer nodes of every
/// enabled zone, which is what `all` means for this member ("an Availability Zone or `all`.
/// This determines whether the target receives traffic from the load balancer nodes in the
/// specified Availability Zone or from all enabled Availability Zones for the load
/// balancer").
pub const ALL_AVAILABILITY_ZONES: &str = "all";

/// The Availability Zone a registration is STORED under, and therefore the one every read of
/// it reports.
///
/// A target whose zone the group's network decides takes the zone that was resolved for it —
/// "if the target type is `ip` and the IP address is in a subnet of the VPC for the target
/// group, the Availability Zone is automatically detected" — else the zone the request named
/// for it. A target of a `lambda` group is in NO such zone and has only one supported value
/// for the member ([`ALL_AVAILABILITY_ZONES`]), so a registration of one carries that value
/// whether or not the request named it: the member is not a caller-chosen setting there but
/// the one fact the target type fixes.
pub fn registered_availability_zone(
    target_type: &str,
    resolved: Option<String>,
    declared: Option<&str>,
) -> Option<String> {
    if target_type == LAMBDA_TARGET_TYPE {
        return Some(ALL_AVAILABILITY_ZONES.to_string());
    }
    resolved.or_else(|| declared.map(str::to_string))
}

impl RegisteredTarget {
    /// Whether the workload behind this registration answers on `port` \u2014 `None` when
    /// nothing declares which ports it serves ([`ServedPorts`]), which is no answer either
    /// way rather than a port it does not serve.
    ///
    /// The one reading of [`RegisteredTarget::serves_ports`], so the health check and any
    /// later reader of a registration's ports agree on what a declared set means.
    pub fn serves(&self, port: i32) -> Option<bool> {
        if self.serves_ports.is_empty() {
            return None;
        }
        Some(self.serves_ports.iter().any(|range| range.contains(&port)))
    }
}

impl TargetGroupRecord {
    /// Health checking on this group's registrations BEGINS now: every registration it
    /// already holds owes its move out of `initial` a settling span from this instant, and
    /// its FIRST check is owed at it.
    ///
    /// Called where a target group becomes one a load balancer forwards to, which is what
    /// starts the health checks: before that nothing is checking these targets, so their
    /// reported state is `unused` rather than a health, and the `initial` their first
    /// check runs under has not started. A registration that is DRAINING is not checked
    /// again — it is leaving the group — so its spans are left alone.
    ///
    /// The first check is owed at the instant checking begins rather than one interval later:
    /// `HealthCheckIntervalSeconds` is "the approximate amount of time, in seconds, between
    /// health checks of an individual target", the span between two of them, and a load
    /// balancer begins sending a registration checks as soon as it begins checking it — which
    /// is the very `initial` `TargetHealth$Reason` words as it sending "the minimum number of
    /// health checks required to determine its health status". So a registration does not wait
    /// a whole cadence for the first finding about it, and every later check comes one
    /// interval after the last ([`Self::observed_check`]).
    pub fn begin_health_checking(&mut self, now: SystemTime) {
        for target in self.targets.values_mut() {
            if target.draining_until.is_none() {
                target.healthy_at = due_at(now, SETTLE_AFTER);
                target.checks_due_at = now;
            }
        }
    }

    /// Re-base every pending check of this group on the interval it NOW carries: no
    /// registration waits longer than one of them from `now`.
    ///
    /// Called where `ModifyTargetGroup` stores a new `HealthCheckIntervalSeconds`. The
    /// cadence a caller configures applies from the moment it is configured, so a check
    /// already scheduled under a longer one is brought forward to one new interval from now;
    /// a check due sooner than that keeps its instant, because the interval is how far apart
    /// checks come at most and a modify does not postpone one the group already owes.
    pub fn reschedule_checks(&mut self, now: SystemTime) {
        let due = due_at(now, health_check_interval(self));
        for target in self.targets.values_mut() {
            if target.draining_until.is_none() {
                target.checks_due_at = target.checks_due_at.min(due);
            }
        }
    }

    /// Fold what ONE look at the registration `key` names observed into its probe, and move
    /// that registration on to its next check.
    ///
    /// A look taken once the group's own cadence has made a check due IS that check: its next
    /// one comes due one `HealthCheckIntervalSeconds` later ([`health_check_interval`]), so
    /// `UnhealthyThresholdCount` consecutive failures take that many intervals to be in, and
    /// what the check observed is counted toward the verdict. A look taken between two checks
    /// counts toward no verdict and only keeps the condition it found ([`TargetProbe::saw`]),
    /// which is what the verdict those checks reach is then reported with.
    ///
    /// `observed` is `None` for a check that observed NOTHING — a registration this reading
    /// performs no check of at all ([`super::host_counts`]). The cadence moves on, and the
    /// probe keeps the verdict it has: nothing is failed on silence, and nothing is found
    /// healthy on it either.
    ///
    /// A key this group no longer holds is skipped: the look ran with no guard held, so a
    /// deregistration may have taken that registration out in the meantime.
    pub fn observed_check(&mut self, key: &TargetKey, observed: Option<Observed>, now: SystemTime) {
        let interval = health_check_interval(self);
        let (healthy_threshold, unhealthy_threshold) = (
            self.health_check.healthy_threshold,
            self.health_check.unhealthy_threshold,
        );
        let Some(target) = self.targets.get_mut(key) else {
            return;
        };
        let due = now >= target.checks_due_at;
        if due {
            target.checks_due_at = due_at(now, interval);
        }
        let Some(observed) = observed else {
            return;
        };
        if due {
            target
                .probe
                .counted(observed, healthy_threshold, unhealthy_threshold);
        } else {
            target.probe.saw(observed);
        }
    }

    /// When the next check of one of this group's registrations comes due, as the span from
    /// `now` — `None` for a group holding no registration to check.
    ///
    /// What the one publisher clock reads so that it wakes for a check rather than sleeping
    /// past it ([`super::host_counts`]): a verdict is a number of checks away, so a check
    /// that is due has to be taken for the verdict to arrive on the cadence the caller
    /// configured. A check already past its due instant is owed at once, which is a span of
    /// no time.
    pub fn next_check_in(&self, now: SystemTime) -> Option<Duration> {
        self.targets
            .values()
            .map(|target| {
                target
                    .checks_due_at
                    .duration_since(now)
                    .unwrap_or(Duration::ZERO)
            })
            .min()
    }

    /// Deregister the registration `key` names: the state transition
    /// `DeregisterTargets` performs.
    ///
    /// A registration a load balancer is sending traffic to enters `draining` and stays in
    /// the group until [`deregistration_delay`] has passed — "the amount of time ... to
    /// wait before changing the state of a deregistering target from `draining` to
    /// `unused`" — which is the contract a client that drains connections before tearing
    /// a target down waits on. One nothing is sending traffic to (a target group no load
    /// balancer forwards to, or a drain configured to take no time) has no connections to
    /// drain, so it leaves the group at once. A deregistration asked for twice keeps the
    /// instant the first one set: the drain does not restart.
    pub fn deregister(&mut self, key: &TargetKey, now: SystemTime) {
        let drains_for = if self.load_balancer_arns.is_empty() {
            Duration::ZERO
        } else {
            deregistration_delay(self)
        };
        if drains_for.is_zero() {
            self.targets.remove(key);
            return;
        }
        if let Some(target) = self.targets.get_mut(key) {
            if target.draining_until.is_none() {
                target.draining_until = Some(due_at(now, drains_for));
            }
        }
    }

    /// The registrations of this group that another emulated service's SCHEDULER owns,
    /// each as the key it is stored under and the workload it stands for
    /// ([`RegisteredTarget::registered_for`]).
    ///
    /// What a scheduler reconciling its workload's registrations may act on: a
    /// registration a `RegisterTargets` caller made is absent from this, so a reconcile
    /// cannot take a caller's own target out of the group. It answers EVERY scheduler's,
    /// because this service cannot know whose a workload is — the caller filters the answer
    /// to the workloads it owns, so two schedulers sharing one group leave each other's
    /// registrations alone.
    pub fn scheduler_registrations(&self) -> Vec<(TargetKey, String)> {
        self.targets
            .iter()
            .filter_map(|(key, target)| {
                target
                    .registered_for
                    .as_ref()
                    .map(|workload| (key.clone(), workload.clone()))
            })
            .collect()
    }

    /// Register the target `declared` describes, on behalf of the emulated service whose
    /// scheduler owns the workload it stands for — the Amazon ECS service registering a task
    /// its `loadBalancers` bind.
    ///
    /// The registration is this group's own in every respect a reader sees: it is keyed and
    /// health-checked exactly as `RegisterTargets` keys and checks one, so it begins
    /// `initial` and is reported whatever its checks then determine
    /// ([`TargetGroupRecord::health_of`]). It carries the workload it stands for, which is
    /// what lets the scheduler that added it take it out again.
    ///
    /// A registration IN SERVICE is not replaced: a key this group already holds — whether a
    /// caller registered it or the same workload did — is left exactly as it is, so a
    /// reconcile does not restart a health check. One that is DRAINING is returned to
    /// service, exactly as `RegisterTargets` returns one (`crate::ops::register_targets`):
    /// `draining` is the state of a target "in the process of being deregistered and the
    /// deregistration delay period has not expired" (`TargetHealth$Reason`
    /// `Target.DeregistrationInProgress`), which a target registered again is not — so the
    /// deregistration is cancelled and the registration is checked from now, rather than
    /// waiting out a drain it is no longer in and leaving the workload reachable through
    /// nothing in the meantime. A group already holding its quota of targets
    /// ([`targets_per_target_group`]) takes no further registration, the same bound
    /// `RegisterTargets` refuses past; a key already held takes up no further room, so
    /// returning one to service is not held to that bound.
    ///
    /// Answers whether this call put a registration in service.
    pub fn register_for(&mut self, declared: &SchedulerRegistration, now: SystemTime) -> bool {
        let key: TargetKey = (declared.id.clone(), declared.port.or(self.port));
        match self.targets.get(&key) {
            // Being deregistered: this call cancels that, and the record below replaces it.
            Some(held) if held.draining_until.is_some() => {}
            Some(_) => return false,
            None if self.targets.len() >= targets_per_target_group(self) => return false,
            None => {}
        }
        // The zone this registration is stored under, read off the group before it is
        // written to: a target type whose targets are in no one zone of the group's network
        // carries the one value the member supports there
        // ([`registered_availability_zone`]).
        let availability_zone = registered_availability_zone(
            &self.target_type,
            declared.availability_zone.clone(),
            None,
        );
        self.targets.insert(
            key.clone(),
            RegisteredTarget {
                id: key.0,
                port: key.1,
                availability_zone,
                registered_at: now,
                healthy_at: due_at(now, SETTLE_AFTER),
                // This record REPLACES whatever the key held, so a target that was being
                // deregistered is no longer in a drain and no longer leaves the group when
                // that drain runs out.
                draining_until: None,
                // The first check of it is owed at once — a load balancer begins sending a
                // registration checks as soon as it is registered — and every later one comes
                // one health-check interval after the last.
                checks_due_at: now,
                probe: TargetProbe::default(),
                registered_for: Some(declared.workload.clone()),
                serves_ports: ServedPorts::clone(&declared.serves_ports),
                guarded_by: GuardingGroups::clone(&declared.guarded_by),
            },
        );
        true
    }

    /// Materialise what the clock already owes this group: a registration whose draining
    /// span is over has LEFT the target group, whether or not anything called in the
    /// meantime.
    ///
    /// Called by the operations that read or write this group's registrations, so the
    /// removal a deregistration already made due is what a later caller sees. It owes the
    /// same removal at the same instant to every caller, so this moves no state a call of
    /// its own would not have moved.
    pub fn settle_draining(&mut self, now: SystemTime) {
        self.targets
            .retain(|_, target| target.draining_until.is_none_or(|due| now < due));
    }

    /// The health of one registered target, derived from what is stored plus the clock:
    /// a deregistering target is draining, a target group no load balancer forwards to
    /// has nothing checking its targets, a group whose health checks are off reports them
    /// unavailable, a registration whose checking has not settled yet is still coming
    /// up, and one whose health the checks have not determined yet is still being checked.
    pub fn health_of(&self, target: &RegisteredTarget, now: SystemTime) -> TargetHealth {
        if let Some(due) = target.draining_until {
            // The drain the caller asked for, until the instant it is over — past which
            // the registration has left the group ([`Self::settle_draining`]) and is no
            // registration of this group at all.
            return reported(if now < due { DRAINING } else { UNUSED_NOT_REGISTERED });
        }
        if self.load_balancer_arns.is_empty() {
            return reported(UNUSED_NOT_IN_USE);
        }
        if !self.health_check.enabled {
            return reported(UNAVAILABLE);
        }
        if now < target.healthy_at {
            return reported(INITIAL);
        }
        if target.probe.is_unhealthy() {
            // The checks have run and this registration did not pass them: the load balancer
            // considers a target unhealthy once the group's threshold of consecutive failures
            // is reached, and `TargetHealth$Reason` words the condition those checks found —
            // an answer the group's success criterion rejects, or a check nothing answered, as
            // the family of load balancer checking it words it ([`unhealthy_report`]).
            return unhealthy_report(self.family(), target.probe.failed_with());
        }
        if target.probe.is_undetermined() {
            // The load balancer has not determined this registration's health: no check of it
            // has found it passing — none has been made yet, the ones made have failed without
            // reaching the group's threshold, or none could be made at all. `healthy` is a
            // finding of the checks, so until one has found it the state is the `initial`
            // `TargetHealth$Reason` words as the minimum number of checks still being sent,
            // and a registration whose checks are failing is not reported healthy on the way
            // to its verdict.
            return reported(INITIAL_HEALTH_CHECKING);
        }
        // What the checks FOUND: a registration one of them found passing, which is the only
        // thing this state is reported on.
        TargetHealth {
            state: Some(HEALTHY_STATE.to_string()),
            reason: None,
            description: None,
        }
    }

    /// The description of a target that is not registered with this group.
    ///
    /// It carries the same `HealthCheckPort` a registered target's does
    /// (`TargetGroupRecord::health_check_port_for`): the port is the group's health-check
    /// configuration read against the target the request named, which an unregistered
    /// target has as much as a registered one.
    ///
    /// And the same `Target$Port`: the port the request named, else the target group's own
    /// ("the port on which the targets receive traffic … you can override the port for a
    /// target when you register it"), which is the port the registration this target would
    /// be is keyed by. So a read answers the port the traffic to that target would go to,
    /// whether or not anything is registered on it.
    pub fn unregistered_health_description(
        &self,
        target: &TargetDescription,
        included: IncludedBlocks,
    ) -> TargetHealthDescription {
        TargetHealthDescription {
            target: Some(TargetDescription {
                port: target.port.or(self.port),
                ..target.clone()
            }),
            health_check_port: self.health_check_port_for(target.port),
            target_health: Some(reported(UNUSED_NOT_REGISTERED)),
            // The optional block the request asked for, and the override in effect on the
            // target the request named — neither of which a registration is needed for
            // ([`anomaly_detection`], [`administrative_override`]).
            anomaly_detection: included.reported_anomaly_detection(),
            administrative_override: Some(administrative_override()),
        }
    }

    /// The health description of one registered target.
    ///
    /// `included` is what the request's `Include` options select ([`IncludedBlocks`]), which
    /// decides the optional blocks this description carries; the `AdministrativeOverride` is
    /// not one of those — it is reported on every description
    /// ([`administrative_override`]).
    pub fn health_description_of(
        &self,
        target: &RegisteredTarget,
        now: SystemTime,
        included: IncludedBlocks,
    ) -> TargetHealthDescription {
        TargetHealthDescription {
            target: Some(TargetDescription {
                id: target.id.clone(),
                port: target.port,
                availability_zone: target.availability_zone.clone(),
                quic_server_id: None,
            }),
            health_check_port: self.health_check_port_of(target),
            target_health: Some(self.health_of(target, now)),
            anomaly_detection: included.reported_anomaly_detection(),
            administrative_override: Some(administrative_override()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A target group carrying `configured` as its deregistration delay.
    fn group_with(configured: Option<&str>) -> TargetGroupRecord {
        let mut group = TargetGroupRecord {
            protocol: Some("HTTP".to_string()),
            target_type: "instance".to_string(),
            load_balancer_arns: vec!["an-attached-load-balancer".to_string()],
            ..TargetGroupRecord::default()
        };
        if let Some(configured) = configured {
            group.attributes.insert(
                DEREGISTRATION_DELAY_ATTRIBUTE.to_string(),
                configured.to_string(),
            );
        }
        group
    }

    /// One row of the cadence table: the case, whether a cadence was asked for, the stored
    /// cadence, the stored `(HealthyThresholdCount, UnhealthyThresholdCount)`, and the spacing
    /// the rule gives it.
    type CadenceCase = (&'static str, bool, i32, (i32, i32), Duration);

    /// The most consecutive checks `HealthCheckThresholdCount` may ask for ("the range is
    /// 2-10"), which the rows below take as the longest run a group's threshold can state.
    const MAX_HEALTH_CHECK_THRESHOLD_COUNT: i32 = 10;

    /// RULE: the span between two checks of a registration is the cadence the CALLER asked
    /// the group's checks to run at, held inside the range `HealthCheckIntervalSeconds`
    /// publishes and inside the spacing that keeps the `unhealthy` verdict its consecutive
    /// failures decide observable ([`VERDICT_WITHIN`]); a group no caller asked a cadence of is
    /// checked at the span this emulator owes a transition of its own.
    ///
    /// The rows span what the rule generalises over: whether the cadence was asked for at
    /// all, each published bound of the member, one value past each of them, a figure no write
    /// could have stored, and — crossed with those — how many consecutive failing checks the
    /// group's threshold makes the verdict's run, from the fewest it admits to the most, one
    /// past the range the member publishes, and a count no write could have stored.
    #[test]
    fn the_cadence_is_the_one_asked_for_bounded_by_the_range_and_the_verdicts_own_span() {
        let rows: [CadenceCase; 13] = [
            (
                "no cadence asked for, left at the documented default",
                false,
                30,
                (5, 2),
                SETTLE_AFTER,
            ),
            (
                "no cadence asked for by a group carrying the shortest",
                false,
                5,
                (2, 2),
                SETTLE_AFTER,
            ),
            (
                "the shortest cadence the member admits, at the fewest failing checks the \
                 threshold asks for",
                true,
                MIN_HEALTH_CHECK_INTERVAL_SECONDS,
                (2, 2),
                Duration::from_secs(5),
            ),
            (
                "the shortest cadence the member admits, at the most failing checks the \
                 threshold admits",
                true,
                MIN_HEALTH_CHECK_INTERVAL_SECONDS,
                (2, MAX_HEALTH_CHECK_THRESHOLD_COUNT),
                Duration::from_secs(2),
            ),
            (
                "a ten-second cadence at the fewest failing checks, whose own run fits exactly",
                true,
                10,
                (2, 2),
                Duration::from_secs(10),
            ),
            (
                "the longest cadence the member admits",
                true,
                MAX_HEALTH_CHECK_INTERVAL_SECONDS,
                (2, 2),
                Duration::from_secs(10),
            ),
            (
                "one second under the shortest",
                true,
                MIN_HEALTH_CHECK_INTERVAL_SECONDS - 1,
                (2, 2),
                Duration::from_secs(5),
            ),
            (
                "one second over the longest, at the documented default thresholds",
                true,
                MAX_HEALTH_CHECK_INTERVAL_SECONDS + 1,
                (5, 2),
                Duration::from_secs(10),
            ),
            (
                "a twenty-second cadence at the most failing checks the threshold admits",
                true,
                20,
                (2, MAX_HEALTH_CHECK_THRESHOLD_COUNT),
                Duration::from_secs(2),
            ),
            ("a cadence of no time at all", true, 0, (2, 2), Duration::from_secs(5)),
            ("a cadence no write could have stored", true, -7, (2, 2), Duration::from_secs(5)),
            (
                "a threshold no write could have stored",
                true,
                30,
                (0, -3),
                VERDICT_WITHIN,
            ),
            (
                "a threshold past the range the member publishes",
                true,
                30,
                (2, 1_000),
                Duration::from_millis(20),
            ),
        ];
        for (case, asked_for, stored, (healthy, unhealthy), expected) in rows {
            let mut group = group_with(None);
            group.health_check.interval_configured = asked_for;
            group.health_check.interval_seconds = stored;
            group.health_check.healthy_threshold = healthy;
            group.health_check.unhealthy_threshold = unhealthy;
            let interval = health_check_interval(&group);
            assert_eq!(
                interval, expected,
                "case {case:?}: expected the checks {expected:?} apart, got {interval:?}"
            );
            // The invariant the second bound exists for: the whole run of consecutive failing
            // checks an `unhealthy` verdict takes is inside the span this emulator keeps a
            // verdict observable in — counted the way the probe counts it, so the invariant
            // holds for whatever number the group carries.
            let checks = unhealthy.max(1);
            let run = interval * checks.unsigned_abs();
            assert!(
                run <= VERDICT_WITHIN,
                "case {case:?}: {checks} checks {interval:?} apart take {run:?}, past the \
                 {VERDICT_WITHIN:?} a verdict stays observable in"
            );
        }
    }

    /// The drain is the configured value, bounded by the attribute's documented range —
    /// so a figure no write of the attribute could have stored cannot become a span, and
    /// no value makes the clock arithmetic leave what it can represent.
    #[test]
    fn the_drain_is_the_configured_value_bounded_by_the_attributes_own_range() {
        let ceiling = u64::try_from(
            target_group_attribute(DEREGISTRATION_DELAY_ATTRIBUTE)
                .and_then(|spec| spec.domain.integer_ceiling())
                .unwrap_or_default(),
        )
        .unwrap_or_default();

        for (case, configured, expected) in [
            ("nothing configured", None, Some(Duration::from_secs(300))),
            ("no delay", Some("0"), Some(Duration::ZERO)),
            ("one second", Some("1"), Some(Duration::from_secs(1))),
            ("the range's ceiling", Some("3600"), None),
            ("one past the ceiling", Some("3601"), None),
            ("the largest figure a u64 carries", Some("18446744073709551615"), None),
            ("a figure no number parses", Some("not-a-number"), Some(Duration::ZERO)),
        ] {
            let expected = expected.unwrap_or(Duration::from_secs(ceiling));
            let group = group_with(configured);
            let delay = deregistration_delay(&group);
            assert_eq!(
                delay, expected,
                "case {case:?}: expected {expected:?}, got {delay:?}"
            );
            // Whatever the figure, the instant the drain comes due is representable.
            let now = SystemTime::now();
            assert!(
                due_at(now, delay) >= now,
                "case {case:?}: the due instant must be representable, got {:?}",
                due_at(now, delay)
            );
        }
    }

    /// A target group the attribute does not apply to drains for no time at all.
    #[test]
    fn a_lambda_target_group_drains_for_no_time() {
        let mut group = group_with(Some("3600"));
        group.target_type = "lambda".to_string();
        group.protocol = None;
        let delay = deregistration_delay(&group);
        assert_eq!(
            delay,
            Duration::ZERO,
            "case a lambda target group: the attribute is not supported for it, got {delay:?}"
        );
    }

    /// RULE: the cadence a caller CHANGES applies from the moment it is changed — a check
    /// scheduled under a longer interval is brought forward to one new interval from now, and
    /// one already due sooner than that keeps the instant the group owes it.
    ///
    /// The rows span what the re-basing generalises over: a pending check further off than the
    /// new interval, one exactly one new interval off, one already due, and a draining
    /// registration, which is not checked again at all. Each row states the span the next
    /// check must end up at, so the assertion does not re-derive what the code does.
    #[test]
    fn a_changed_interval_rebases_the_pending_checks_and_postpones_none() {
        let now = SystemTime::now();
        let was = Duration::from_secs(300);
        let rows: [(&str, Duration, bool, Duration); 4] = [
            // The old interval put it five minutes off; the new one is five seconds.
            (
                "a check the old interval put five minutes off",
                was,
                false,
                Duration::from_secs(5),
            ),
            (
                "a check one new interval off",
                Duration::from_secs(5),
                false,
                Duration::from_secs(5),
            ),
            ("a check already due", Duration::ZERO, false, Duration::ZERO),
            // Not re-based at all: it is leaving the group.
            ("a draining registration", was, true, was),
        ];
        for (case, pending, draining, expected) in rows {
            let mut group = group_with(None);
            // A cadence the caller ASKED for, which is the one the checks are spaced by.
            group.health_check.interval_configured = true;
            group.health_check.interval_seconds = 300;
            let key: TargetKey = ("10.0.0.7".to_string(), Some(80));
            group.targets.insert(
                key.clone(),
                RegisteredTarget {
                    id: key.0.clone(),
                    port: key.1,
                    availability_zone: None,
                    registered_at: now,
                    healthy_at: now,
                    draining_until: draining.then(|| due_at(now, was)),
                    checks_due_at: due_at(now, pending),
                    probe: TargetProbe::default(),
                    registered_for: None,
                    serves_ports: Vec::new().into(),
                    guarded_by: Vec::new().into(),
                },
            );
            // The modify stores the new interval, then re-bases what is pending.
            group.health_check.interval_seconds = 5;
            group.reschedule_checks(now);

            let due = group
                .targets
                .get(&key)
                .expect("the registration is held")
                .checks_due_at;
            let expected = due_at(now, expected);
            assert_eq!(
                due, expected,
                "case {case:?}: expected the next check at {expected:?}, got {due:?}"
            );
        }
    }

    /// RULE: an `unhealthy` verdict is WORDED by the condition its last failing check found —
    /// an answer the group's criterion rejects as the model's response-code mismatch, described
    /// by the status that answer carried; an error or an answer that is no response as the
    /// model's failed checks, whatever family performed them; and a check nothing answered as
    /// the family's own wording of that condition.
    ///
    /// The rows span every condition a check can fail on, each read for every family of load
    /// balancer, and the answered ones over several statuses — so the description is the code
    /// the target really answered rather than a constant.
    #[test]
    fn an_unhealthy_verdict_is_worded_by_the_condition_its_checks_found() {
        /// The three members one reported health carries, which is what a row compares.
        fn members(health: &TargetHealth) -> (&str, &str, &str) {
            (
                health.state.as_deref().unwrap_or_default(),
                health.reason.as_deref().unwrap_or_default(),
                health.description.as_deref().unwrap_or_default(),
            )
        }
        let families = [
            (
                "an application load balancer",
                super::super::LoadBalancerFamily::Application,
            ),
            (
                "a network load balancer",
                super::super::LoadBalancerFamily::Network,
            ),
            (
                "a gateway load balancer",
                super::super::LoadBalancerFamily::Gateway,
            ),
        ];
        for (family_case, family) in families {
            // A check the target ANSWERED: one reason, and a description naming that answer's
            // own status, for every family.
            for status in [500_u16, 503, 404, 301] {
                let report = unhealthy_report(family, Some(CheckFailure::ResponseCode(status)));
                let described = response_code_mismatch_description(status);
                let expected = (UNHEALTHY_STATE, RESPONSE_CODE_MISMATCH_REASON, described.as_str());
                assert_eq!(
                    members(&report),
                    expected,
                    "case {family_case:?} answered {status}: expected {expected:?}, got {:?}",
                    members(&report)
                );
            }
            // A check that came back with an error or an answer that is no response: the
            // model's own reason for that condition, with no code to mismatch.
            let report = unhealthy_report(family, Some(CheckFailure::Malformed));
            assert_eq!(
                members(&report),
                UNHEALTHY_FAILED_CHECKS,
                "case {family_case:?} an error in place of a response: expected \
                 {UNHEALTHY_FAILED_CHECKS:?}, got {:?}",
                members(&report)
            );
            // A check nothing answered, and a verdict whose failures are no longer known: the
            // family's own wording.
            for (case, failure) in [
                ("a check nothing answered", Some(CheckFailure::Unanswered)),
                ("a verdict whose failures are unknown", None),
            ] {
                let report = unhealthy_report(family, failure);
                let expected = unhealthy_for(family);
                assert_eq!(
                    members(&report),
                    expected,
                    "case {family_case:?} {case}: expected {expected:?}, got {:?}",
                    members(&report)
                );
            }
        }
    }

    /// RULE: a registration leaves `initial` only once the CHECKS have determined its health —
    /// `healthy` once one of them has found it passing, `unhealthy` once as many consecutive
    /// checks failed as the group's threshold takes — so a registration no check has been made
    /// of, one whose checks have only failed, and one whose checks observed nothing at all are
    /// reported as the load balancer still sending the checks its health is determined from;
    /// and a health the checks reached is not given back to `initial`.
    ///
    /// The rows span what the rule generalises over: how many checks were taken, what each
    /// found (an answer the criterion rejects, nothing at all, an answer it accepts), the order
    /// they came in, and whether a look between two checks saw a failure. The clock each row
    /// reads at is PASSED IN, so no row waits for one.
    #[test]
    fn a_registration_is_healthy_only_once_a_check_has_found_it_so() {
        /// The pair a row compares: the state and the reason it is reported under.
        fn reported_pair(health: &TargetHealth) -> (&str, &str) {
            (
                health.state.as_deref().unwrap_or_default(),
                health.reason.as_deref().unwrap_or_default(),
            )
        }
        let failed = Observed::Failed(CheckFailure::ResponseCode(500));
        let passed = Observed::Passed;
        let initial_checking = (INITIAL_STATE, INITIAL_HEALTH_CHECKING.1);
        let healthy = (HEALTHY_STATE, "");
        let unhealthy = (UNHEALTHY_STATE, RESPONSE_CODE_MISMATCH_REASON);
        // Each row: the checks the clock took, the looks taken between checks, and the pair the
        // registration must then be reported as. A threshold of two checks either way.
        type Row<'a> = (
            &'a str,
            &'a [Option<Observed>],
            &'a [Observed],
            (&'a str, &'a str),
        );
        let rows: [Row<'_>; 11] = [
            ("no check taken at all", &[], &[], initial_checking),
            (
                "one failing check of the two a verdict takes",
                &[Some(failed)],
                &[],
                initial_checking,
            ),
            (
                "the threshold of failing checks",
                &[Some(failed), Some(failed)],
                &[],
                unhealthy,
            ),
            (
                "one check that observed nothing at all",
                &[None],
                &[],
                initial_checking,
            ),
            (
                "more checks that observed nothing than either threshold takes",
                &[None, None, None],
                &[],
                initial_checking,
            ),
            (
                "checks that observed nothing and then one that found it passing",
                &[None, None, Some(passed)],
                &[],
                healthy,
            ),
            (
                "checks that observed nothing and then the threshold of failing ones",
                &[None, Some(failed), Some(failed)],
                &[],
                unhealthy,
            ),
            (
                "one passing check of the two a healthy verdict takes",
                &[Some(passed)],
                &[],
                healthy,
            ),
            (
                "a passing check and then a failing one, short of the threshold",
                &[Some(passed), Some(failed)],
                &[],
                healthy,
            ),
            (
                "no check taken, and a look between checks that failed",
                &[],
                &[failed],
                initial_checking,
            ),
            (
                "a passing check, and a look between checks that failed",
                &[Some(passed)],
                &[failed],
                healthy,
            ),
        ];
        for (case, checks, looks, expected) in rows {
            let mut group = group_with(None);
            group.health_check.enabled = true;
            // A cadence the caller ASKED for, which is the one the checks are spaced by.
            group.health_check.interval_configured = true;
            group.health_check.interval_seconds = 5;
            group.health_check.healthy_threshold = 2;
            group.health_check.unhealthy_threshold = 2;
            let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
            let key: TargetKey = ("10.0.0.9".to_string(), Some(80));
            group.targets.insert(
                key.clone(),
                RegisteredTarget {
                    id: key.0.clone(),
                    port: key.1,
                    availability_zone: None,
                    registered_at: now,
                    // The registration span is behind it, so what is read is the health its
                    // checks decided rather than the registration being made.
                    healthy_at: now,
                    draining_until: None,
                    checks_due_at: now,
                    probe: TargetProbe::default(),
                    registered_for: None,
                    serves_ports: Vec::new().into(),
                    guarded_by: Vec::new().into(),
                },
            );
            let mut at = now;
            for observed in checks {
                group.observed_check(&key, *observed, at);
                at += health_check_interval(&group);
            }
            // A look taken BETWEEN two checks: folded against an instant before the next check
            // of the registration is due, so it counts toward no verdict.
            for observed in looks {
                group.observed_check(&key, Some(*observed), at - Duration::from_secs(1));
            }
            let target = group.targets.get(&key).expect("the registration is held");
            let health = group.health_of(target, at);
            assert_eq!(
                reported_pair(&health),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                reported_pair(&health)
            );
        }
    }

    /// A scheduler's declaration of one registration: `id` reached on `port`, standing for
    /// `workload`.
    fn declared_by_a_scheduler(
        id: &str,
        port: Option<i32>,
        workload: &str,
    ) -> SchedulerRegistration {
        SchedulerRegistration {
            id: id.to_string(),
            port,
            availability_zone: None,
            workload: workload.to_string(),
            serves_ports: Vec::new().into(),
            guarded_by: Vec::new().into(),
        }
    }

    /// What one key of a group holds before the scheduler registers it again.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Held {
        /// Nothing: the key is free.
        Nothing,
        /// A registration IN SERVICE, made by the same scheduler.
        InServiceForAScheduler,
        /// A registration IN SERVICE that a `RegisterTargets` caller made.
        InServiceForACaller,
        /// A registration being DEREGISTERED, made by the same scheduler.
        DrainingForAScheduler,
        /// A registration being DEREGISTERED that a `RegisterTargets` caller made.
        DrainingForACaller,
    }

    /// RULE: a scheduler registering a target puts it IN SERVICE — the registration is
    /// reported `initial` while its first checks are sent and carries the workload it stands
    /// for — whether that key was free or held a registration being DEREGISTERED, because
    /// `draining` is the state of a target "in the process of being deregistered and the
    /// deregistration delay period has not expired" (`TargetHealth$Reason`
    /// `Target.DeregistrationInProgress`), which a target registered again is not; a key
    /// holding a registration that is in service is left exactly as it is, so a reconcile
    /// that moved nothing restarts no check.
    ///
    /// The rows span what the rule generalises over: what the key held (nothing, a
    /// registration in service, one being deregistered) and WHOSE it was (the same
    /// scheduler's, and a `RegisterTargets` caller's), each at the group's own port and at a
    /// port the registration overrides.
    #[test]
    fn a_scheduler_registering_a_draining_target_returns_it_to_service() {
        let rows: [(&str, Held, bool); 5] = [
            ("a key the group holds nothing at", Held::Nothing, true),
            (
                "a registration of the same workload that is in service",
                Held::InServiceForAScheduler,
                false,
            ),
            (
                "a registration a RegisterTargets caller made and is in service",
                Held::InServiceForACaller,
                false,
            ),
            (
                "a registration of an earlier workload that is being deregistered",
                Held::DrainingForAScheduler,
                true,
            ),
            (
                "a registration a RegisterTargets caller made and is deregistering",
                Held::DrainingForACaller,
                true,
            ),
        ];
        // The ports the registration may receive traffic on: the group's own, and one the
        // registration overrides ("you can override the port for a target when you register
        // it", `RegisterTargets`).
        let ports: [(&str, Option<i32>); 2] = [
            ("the group's own port", None),
            ("an overridden port", Some(8080)),
        ];
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let address = "10.0.0.9";
        let earlier = "arn:aws:ecs:us-east-1:000000000000:task/served/earlier";
        let placed = "arn:aws:ecs:us-east-1:000000000000:task/served/placed";
        for (port_case, port) in ports {
            for (case, held, expected_in_service) in rows {
                let case = format!("{case} at {port_case}");
                let mut group = group_with(None);
                group.port = Some(80);
                group.health_check.enabled = true;
                let key: TargetKey = (address.to_string(), port.or(group.port));
                let draining_before =
                    matches!(held, Held::DrainingForAScheduler | Held::DrainingForACaller);
                let scheduler_owned_before = matches!(
                    held,
                    Held::InServiceForAScheduler | Held::DrainingForAScheduler
                );
                if held != Held::Nothing {
                    let drain = due_at(now, deregistration_delay(&group));
                    group.targets.insert(
                        key.clone(),
                        RegisteredTarget {
                            id: key.0.clone(),
                            port: key.1,
                            availability_zone: None,
                            registered_at: now,
                            healthy_at: now,
                            draining_until: draining_before.then_some(drain),
                            checks_due_at: now,
                            probe: TargetProbe::default(),
                            registered_for: scheduler_owned_before
                                .then(|| earlier.to_string()),
                            serves_ports: Vec::new().into(),
                            guarded_by: Vec::new().into(),
                        },
                    );
                }

                let answered = group.register_for(
                    &declared_by_a_scheduler(address, port, placed),
                    now + Duration::from_secs(1),
                );
                assert_eq!(
                    answered, expected_in_service,
                    "case {case:?}: expected register_for to answer {expected_in_service}, got \
                     {answered}"
                );

                let target = group
                    .targets
                    .get(&key)
                    .unwrap_or_else(|| panic!("case {case:?}: the group holds the registration"));
                let state = group
                    .health_of(target, now + Duration::from_secs(1))
                    .state
                    .unwrap_or_default();
                if expected_in_service {
                    assert_eq!(
                        state, INITIAL_STATE,
                        "case {case:?}: a registration put in service is checked from now, so it \
                         is reported {INITIAL_STATE}, got {state:?}"
                    );
                    assert_eq!(
                        target.registered_for.as_deref(),
                        Some(placed),
                        "case {case:?}: the registration stands for the workload this call \
                         declared, got {:?}",
                        target.registered_for
                    );
                    assert_eq!(
                        target.draining_until, None,
                        "case {case:?}: a registration in service is in no drain, got {:?}",
                        target.draining_until
                    );
                } else {
                    assert_eq!(
                        target.registered_at, now,
                        "case {case:?}: a registration already in service keeps the instant it \
                         was made at, got {:?}",
                        target.registered_at
                    );
                    // Left exactly as it is: whose it is, and the checking it is already
                    // under, are untouched by a registration this call did not make.
                    assert_eq!(
                        target.registered_for.as_deref(),
                        scheduler_owned_before.then_some(earlier),
                        "case {case:?}: a registration already in service keeps the workload it \
                         stood for, got {:?}",
                        target.registered_for
                    );
                    assert_eq!(
                        target.checks_due_at, now,
                        "case {case:?}: a registration already in service keeps the instant its \
                         next check is due at, got {:?}",
                        target.checks_due_at
                    );
                    assert_ne!(
                        state, DRAINING.0,
                        "case {case:?}: a registration already in service is in no drain, got \
                         {state:?}"
                    );
                }
            }
        }
    }
}
