// SPDX-License-Identifier: CC0-1.0

//! The user interface ONE session serves, and the ARN that addresses it — what
//! `GetResourceDashboard` answers for.
//!
//! One concern of this service's state, in two halves:
//!
//! * WHICH session an ARN names ([`session_arn_reference`] and [`addressed_session`]).
//!   `GetResourceDashboardRequest$ResourceARN` is documented "The Amazon Resource Name (ARN)
//!   for a session", so the resource the call addresses is a session of this account — read
//!   out of the ARN the caller submitted and resolved against the one [`SessionRegistry`]
//!   every other session-addressed operation reads, rather than against a second copy. The
//!   ARN is read through [`::service_common::arn`], the one place this tree parses a
//!   caller-supplied ARN. The reading is in two steps, because the two failures are
//!   client-visibly different: whether the value spells out the ARN of a session AT ALL is
//!   the request's own FORM ([`session_arn_reference`],
//!   [`RESOURCE_ARN_NOT_COMPLETE_OR_INVALID`]), and whether the session it names is one this
//!   account holds is RESOLUTION ([`addressed_session`], [`RESOURCE_ARN_NOT_FOUND`]).
//! * WHAT that session's interface is ([`SessionRecord::dashboard_url`]).
//!   `GetResourceDashboardResponse$Url` is documented "The Live UI/Persistence UI url for a
//!   session": those are the two interfaces a Spark session has, and WHICH of them the URL
//!   addresses follows the session's own state rather than the request — a session whose
//!   resources are running serves its live interface, and one whose resources "are no longer
//!   running" (`SessionStatus$State`, the TERMINATED state) is served by the persistent
//!   interface that outlives them. So a dashboard is readable for as long as the record is,
//!   and the address a caller is handed changes when the session does.
//!
//! The URL is PRESIGNED against this emulator's own signer ([`session_dashboard_url`]), so a
//! caller that follows it reaches the emulator that answered it rather than an address
//! nothing serves, and it stops working once the validity it was signed for has passed.
//! Everything in it but the signature is derived from the session the ARN resolved to, so
//! two reads of one session's dashboard address the same interface.
//!
//! Nothing here writes: a dashboard is not a resource of its own but a projection of a
//! session, so reading one leaves the registry exactly as the next caller would have found
//! it.

use std::time::{Duration, SystemTime};

use super::*;

/// The resource type an Athena SESSION is addressed by inside an ARN.
///
/// Athena's own resource-type table publishes `workgroup`, `datacatalog` and
/// `capacity-reservation` (see [`WORK_GROUP_RESOURCE_TYPE`] and its neighbours) and no
/// type for a session, which is addressed by its `SessionId` everywhere else in this API.
/// So the session an ARN names is read from the resource field's own tail: this token where
/// the ARN names the session directly (`session/<SessionId>`), and the session segment of a
/// workgroup-scoped ARN (`workgroup/<WorkGroupName>/session/<SessionId>`), which is the
/// form that spells out the workgroup a session belongs to ([`SessionRecord::work_group`]).
pub const SESSION_RESOURCE_TYPE: &str = "session";

/// The message a dashboard read answers an ARN it resolved no session of this account from
/// with.
///
/// ONE sentence for the whole class of RESOLUTION misses, naming the MEMBER that was wrong
/// and nothing of the value it carried: an ARN naming another vendor's, another Region's or
/// another account's session, and one naming a session this account does not hold, are all
/// answered alike, because what the service reports is that the submitted `ResourceARN`
/// resolved to nothing — and a refusal that quoted the ARN back would disclose which ARNs do
/// resolve. A value that spells out no session ARN at all is the other condition
/// ([`RESOURCE_ARN_NOT_COMPLETE_OR_INVALID`]).
pub const RESOURCE_ARN_NOT_FOUND: &str = "ResourceARN is not found";

/// The message a dashboard read answers a `ResourceARN` out of which no session reference
/// can be read at all.
///
/// Athena's own wording for the input-shape failure, and wholly fixed text (no terminating
/// period): it quotes nothing of the value the request carried. A different condition from
/// [`RESOURCE_ARN_NOT_FOUND`] and reported before it — a value that does not spell out the
/// ARN of a session names no session in any account, so the remedy is to correct the
/// REQUEST, while a value that does spell one out and resolves to nothing says the session it
/// names is absent.
pub const RESOURCE_ARN_NOT_COMPLETE_OR_INVALID: &str = "ResourceARN is not complete or invalid";

/// The session a `ResourceARN` addresses: its id, and the workgroup when the ARN scopes it
/// to one.
///
/// Borrowed from the ARN that was read, so resolving one allocates nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddressedSession<'arn> {
    /// The workgroup the ARN scoped the session to, for the form that names one; `None`
    /// where the ARN names the session alone. A session BELONGS to the workgroup it was
    /// started in, so an ARN naming another workgroup's name addresses no session this
    /// account holds.
    pub work_group: Option<&'arn str>,
    /// The `SessionId` the ARN names — the key the [`SessionRegistry`] holds a session
    /// under.
    pub session_id: &'arn str,
}

/// The session reference the value `arn` spells out, read from its FORM alone, or `None` for
/// a value out of which no session reference can be read at all.
///
/// `ResourceARN` is documented "The Amazon Resource Name (ARN) for a session", so the value
/// has to spell out the ARN of a session completely: the six colon-separated fields of
/// [`::service_common::arn`]'s form with the partition, the vendor, the Region, the account
/// and the resource all filled in — a session is a REGIONAL resource of one account, so
/// every field of the ARN that names one is written — and a resource field naming a session,
/// either directly (`session/<SessionId>`) or scoped to the workgroup the session belongs to
/// (`workgroup/<WorkGroupName>/session/<SessionId>`), with no segment left empty and none
/// past the id.
///
/// WHOSE session that is — the vendor, the account and the Region fields — is not read here:
/// that is resolution, which [`addressed_session`] applies on top of this reading. A value
/// this reads nothing out of is the REQUEST being wrong, however complete the account's
/// session registry is ([`RESOURCE_ARN_NOT_COMPLETE_OR_INVALID`]).
#[must_use]
pub fn session_arn_reference(arn: &str) -> Option<AddressedSession<'_>> {
    session_reference_of(::service_common::arn::parse(arn)?)
}

/// The same reading, from an ARN already parsed — so a caller that has read the other fields
/// of `parsed` does not split the value a second time.
fn session_reference_of<'arn>(
    parsed: ::service_common::arn::Arn<'arn>,
) -> Option<AddressedSession<'arn>> {
    if parsed.region.is_empty() || parsed.account.is_empty() {
        return None;
    }
    // AWS writes a resource in either form, `<type>/<id>` and `<type>:<id>`, so both
    // separators divide the resource field's segments — the division
    // `::service_common::arn::resource_type` already reads the leading token by.
    let segments: Vec<&'arn str> = parsed.resource.split(['/', ':']).collect();
    let addressed = match segments[..] {
        [SESSION_RESOURCE_TYPE, session_id] => AddressedSession {
            work_group: None,
            session_id,
        },
        [WORK_GROUP_RESOURCE_TYPE, work_group, SESSION_RESOURCE_TYPE, session_id] => {
            AddressedSession {
                work_group: Some(work_group),
                session_id,
            }
        }
        _ => return None,
    };
    (!addressed.session_id.is_empty()
        && addressed
            .work_group
            .is_none_or(|work_group| !work_group.is_empty()))
    .then_some(addressed)
}

/// The session the ARN `arn` addresses, where `region` is the Region the CALL was made in
/// (see [`authorization::region_from_request`]), or `None` for an ARN no session of this
/// endpoint resolves from.
///
/// RESOLUTION on top of the reading [`session_arn_reference`] makes: a dashboard read reaches
/// the sessions of the account it was signed for, in the Region it was addressed to, so an
/// ARN naming another vendor's, another account's or another Region's session resolves
/// nothing here — the same reading [`addressed_resource`] makes of a tagging call's ARN,
/// except that "another account" is not told apart from the rest: this operation reports only
/// that the submitted member resolved to no session ([`RESOURCE_ARN_NOT_FOUND`]).
///
/// The partition is not read: one endpoint serves one partition, and the account and Region
/// above already say whether the ARN addresses what this endpoint holds.
///
/// The account and the Region are compared for EQUALITY here, so an empty field matches
/// nothing rather than matching every account or Region: an ARN that leaves either blank is
/// already no session ARN at all ([`session_reference_of`]), and the allowance other services
/// make for a partition-global or account-less resource has no place on a member documented
/// as the ARN of a session.
#[must_use]
pub fn addressed_session<'arn>(arn: &'arn str, region: &str) -> Option<AddressedSession<'arn>> {
    let parsed = ::service_common::arn::parse(arn)?;
    if parsed.vendor != authorization::ACTION_PREFIX {
        return None;
    }
    if parsed.account != ::engine::emulator_account_id() {
        return None;
    }
    if parsed.region != region {
        return None;
    }
    session_reference_of(parsed)
}

/// The path segment the interface of a session whose resources are RUNNING is served on:
/// the live interface of `GetResourceDashboardResponse$Url`'s pair.
pub const LIVE_UI_PATH: &str = "live";

/// The path segment the interface of a session whose resources are GONE is served on: the
/// persistent interface of that same pair, which is what remains to read once a session has
/// been shut down.
pub const PERSISTENCE_UI_PATH: &str = "persistence";

/// How long a dashboard URL this service hands out stays valid.
///
/// A CHOSEN bound: Athena publishes no validity for a session's interface URL, so this is
/// long enough for a caller to open the interface it addresses and short enough that a URL
/// which was cached or shared stops working.
pub const SESSION_DASHBOARD_URL_VALIDITY: Duration = Duration::from_secs(60 * 60);

/// The presigned address of the interface `ui_path` of the session `session_id`.
///
/// The ONE composition of a dashboard address in this service: query-signed by this
/// emulator's own signer for [`SESSION_DASHBOARD_URL_VALIDITY`], so a caller that follows it
/// reaches the emulator that answered, and the URL carries the instant it was signed and how
/// long it stays valid — which is what makes it stop working rather than open an interface
/// for ever.
///
/// `None` when the request cannot be signed, which the answering call reports as its
/// modelled server error rather than an address that verifies nowhere.
#[must_use]
pub fn session_dashboard_url(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    session_id: &str,
    ui_path: &str,
) -> Option<String> {
    let path = format!("/sessions/{session_id}/{ui_path}/");
    let request = ::smithy_server_runtime::SignedUrlRequest::new(
        ::http::Method::GET,
        ctx.identity().signing_name(),
        &path,
    );
    match ctx.url_signer().sign(request, SESSION_DASHBOARD_URL_VALIDITY) {
        Ok(url) => Some(url.as_str().to_string()),
        Err(error) => {
            ::tracing::warn!(error = ?error, "a session dashboard URL could not be presigned");
            None
        }
    }
}

impl SessionRecord {
    /// Which of the two interfaces this session serves at `now`: the live one while its
    /// resources are running, the persistent one once they are gone.
    #[must_use]
    pub fn dashboard_ui_path(&self, now: SystemTime) -> &'static str {
        if self.is_terminated_at(now) {
            PERSISTENCE_UI_PATH
        } else {
            LIVE_UI_PATH
        }
    }

    /// The presigned address of this session's interface at `now`, composed by the one
    /// builder above from the session's OWN id and state.
    #[must_use]
    pub fn dashboard_url(
        &self,
        ctx: &::smithy_server_runtime::OperationContext<'_>,
        now: SystemTime,
    ) -> Option<String> {
        session_dashboard_url(ctx, &self.id, self.dashboard_ui_path(now))
    }
}
