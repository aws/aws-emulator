// SPDX-License-Identifier: CC0-1.0

//! The PAYMENTS plane's state: the payment sessions this service holds.
//!
//! A payment session is MINTED by `CreatePaymentSession` — the call that opens it names
//! the payment manager that owns it, the user it acts for, how long it lives and the
//! spending limits it may draw on, and this emulator mints the `paymentSessionId` it is
//! addressed by ([`mint_payment_session_id`]). `GetPaymentSession` reports one of them
//! back, `ListPaymentSessions` reports the ones a manager owns and `DeletePaymentSession`
//! removes one, so a `paymentSessionId` this plane holds no session for is refused with one
//! sentence ([`payment_session_not_found_message`]).
//!
//! A session belongs to a payment MANAGER, and a manager is created by the AgentCore
//! CONTROL plane: this plane creates none, so whether the account holds the manager a
//! request names is resolved against the component that OWNS it
//! ([`payment_manager_resolved`]) rather than against this plane's own records — the same
//! idiom the browser plane uses for a browser ([`super::browser::browser_availability`]). A
//! request naming a manager that plane holds none of is denied, with the same sentence for
//! every operation of the payments plane ([`PAYMENT_MANAGER_NOT_FOUND_MESSAGE`]).
//!
//! A session records the window the call that opened it asked for
//! (`expiryTimeInMinutes`) and the user it belongs to: `DeletePaymentSession` removes only a
//! session whose user the deleting request agrees with
//! ([`PaymentSessionRecord::acts_for`]), and answers the one `PaymentSessionStatus` the
//! model fixes for a successful hard delete ([`PAYMENT_SESSION_STATUS_DELETED`]).
//!
//! The plane has an explicit ceiling: `CreatePaymentSession` is its growth site and a
//! caller chooses both the manager it creates under and the `clientToken` each insert is
//! recorded with, so it is bounded by [`MAX_PAYMENT_SESSIONS`] and
//! [`MAX_PAYMENT_SESSION_IDEMPOTENCY_RECORDS`], each evicting its least recently written
//! record once that maximum is passed.

use std::time::SystemTime;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol as control_plane;

use crate::ops::types::{
    Amount, AvailableLimits, PaymentSession, PaymentSessionSummary, SessionLimits,
};

use super::ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState;

/// The most payment sessions this emulator retains; the least recently created one is
/// evicted once the store passes it.
///
/// An EMULATOR ceiling, not an AWS quota: every `CreatePaymentSession` appends a record, so
/// without one a caller looping on the create would grow the process heap without end.
pub const MAX_PAYMENT_SESSIONS: usize = 256;

/// The most `CreatePaymentSession` `clientToken` records this emulator retains.
pub const MAX_PAYMENT_SESSION_IDEMPOTENCY_RECORDS: usize = 1_000;

/// `PaymentSessionStatus` a successful delete reports: `DeletePaymentSessionResponse$status`
/// is documented as "the status of the deletion. Always DELETED for successful hard delete"
/// (source: smithy_query query="doc" shape="DeletePaymentSessionResponse" member="status";
/// the enum's values are ACTIVE | EXPIRED | DELETED).
pub const PAYMENT_SESSION_STATUS_DELETED: &str = "DELETED";

/// How this service words the refusal of a request naming a payment manager the account
/// does not hold: a fixed sentence that carries nothing from the request — not the ARN it
/// named, and no per-run id.
///
/// One sentence for every operation of the payments plane, so the same condition cannot be
/// worded two ways.
pub const PAYMENT_MANAGER_NOT_FOUND_MESSAGE: &str = "Payment manager not found";

/// How this service words the `ResourceNotFoundException` a payment-session read or delete
/// is refused with when the manager holds no session of that id: the service's own
/// not-found sentence for the resource that did not resolve
/// ([`super::resource_not_found_message`]), whose only variable part is the
/// `paymentSessionId` the REQUEST named.
///
/// One sentence for both callers — the read that cannot report it and the delete that
/// cannot remove it — so the two cannot word the same condition differently.
pub fn payment_session_not_found_message(payment_session_id: &str) -> String {
    super::resource_not_found_message(super::NotFoundResource::PaymentSession, payment_session_id)
}

/// How this service words the refusal of a delete whose `userId` is not the user the
/// session belongs to: `DeletePaymentSessionRequest$userId` is documented as "the user ID
/// making the delete request. Must match the session's userId", so the sentence states that
/// rule and names the member it failed on.
///
/// It carries nothing from the store: the user a session belongs to is not disclosed to a
/// request that named another.
pub const PAYMENT_SESSION_USER_MISMATCH_MESSAGE: &str = "userId must match the session's userId";

/// The prefix every `paymentSessionId` carries, fixed by `PaymentSessionId`'s pattern
/// `^payment-session-[0-9a-zA-Z-]{15}$` (source: smithy_query query="shape"
/// shape="PaymentSessionId").
pub const PAYMENT_SESSION_ID_PREFIX: &str = "payment-session-";

/// One payment session: what `CreatePaymentSession` persisted, which is what every later
/// read of the session reports.
#[derive(Debug, Clone)]
pub struct PaymentSessionRecord {
    /// Minted by [`mint_payment_session_id`]; never client-supplied.
    pub payment_session_id: String,
    /// The manager that owns this session, as the creating request spelled it.
    pub payment_manager_arn: String,
    /// The user this session acts for: the one the creating request named, and the empty
    /// value when it named none — `UserId` admits one (length ..120, no minimum) while
    /// `PaymentSession$userId` is `@required`, so a session opened without a user is
    /// reported as belonging to none.
    pub user_id: String,
    /// The agent the creating request named, kept because the member is documented as
    /// carrying the agent "associated with this request, used for observability".
    pub agent_name: Option<String>,
    /// The spending limits this session may draw on, or `None` when the creating request
    /// set none.
    pub limits: Option<SessionLimits>,
    /// How many minutes the session lives, as the creating request asked ("the session
    /// expiry time in minutes"), reported back by every read of the session.
    pub expiry_time_in_minutes: i32,
    pub created_at: SystemTime,
    /// When this session was last changed by a call: its creation instant until one does.
    pub updated_at: SystemTime,
    /// The value [`ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState::next_seq`]
    /// handed this session: its eviction order, and what keeps two sessions created in the
    /// same millisecond apart.
    pub seq: u64,
}

impl PaymentSessionRecord {
    /// A session as the call that opens it leaves it: the window and the limits that call
    /// asked for, created at `created_at`.
    pub fn created(
        seq: u64,
        payment_manager_arn: String,
        user_id: String,
        agent_name: Option<String>,
        limits: Option<SessionLimits>,
        expiry_time_in_minutes: i32,
        created_at: SystemTime,
    ) -> Self {
        Self {
            payment_session_id: mint_payment_session_id(created_at, seq),
            payment_manager_arn,
            user_id,
            agent_name,
            limits,
            expiry_time_in_minutes,
            created_at,
            // Nothing has changed this session yet, so the last instant it changed is the
            // instant it came into being.
            updated_at: created_at,
            seq,
        }
    }

    /// Whether this session is one a request naming `user_id` may act on: the user the
    /// session belongs to is the one it was opened for, and `DeletePaymentSessionRequest`'s
    /// own `userId` "must match the session's userId".
    ///
    /// The member is optional, so a request that names NO user disagrees with none; only a
    /// value that differs from the session's is a mismatch.
    pub fn acts_for(&self, user_id: Option<&str>) -> bool {
        user_id.is_none_or(|named| self.user_id == named)
    }

    /// What this session still has left to spend: the maximum its limits set, less what it
    /// has already spent, as of the last instant that changed.
    ///
    /// A session with no limits has no available-limits report of its own, which is why the
    /// modelled member is optional while `SessionLimits$maxSpendAmount` is required.
    pub fn available_limits(&self) -> Option<AvailableLimits> {
        let limits = self.limits.as_ref()?;
        Some(AvailableLimits {
            available_spend_amount: Some(Amount {
                // Nothing has been charged against this session, so the whole maximum is
                // still available. The value is carried as the decimal STRING the limit was
                // set with, so no precision of it is lost on the way back out.
                value: limits.max_spend_amount.value.clone(),
                currency: limits.max_spend_amount.currency.clone(),
            }),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
        })
    }

    /// This session as the modelled `PaymentSession`, which is what the create and every
    /// read of the session answer with.
    pub fn session(&self) -> PaymentSession {
        PaymentSession {
            payment_session_id: self.payment_session_id.clone(),
            payment_manager_arn: self.payment_manager_arn.clone(),
            limits: self.limits.clone(),
            user_id: self.user_id.clone(),
            expiry_time_in_minutes: self.expiry_time_in_minutes,
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            available_limits: self.available_limits(),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
        }
    }

    /// This session as the modelled `PaymentSessionSummary`, which is what a listing
    /// reports: the session's identity and window, without the limits a read of the one
    /// session carries.
    pub fn summary(&self) -> PaymentSessionSummary {
        PaymentSessionSummary {
            payment_session_id: self.payment_session_id.clone(),
            payment_manager_arn: self.payment_manager_arn.clone(),
            user_id: self.user_id.clone(),
            expiry_time_in_minutes: self.expiry_time_in_minutes,
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
        }
    }

    /// Whether this session is one the payment manager `payment_manager_arn` owns.
    ///
    /// The ARN is compared WHOLE, the way the manager's own existence is resolved: it names
    /// the partition, region and account the manager lives in, so an ARN that agrees only
    /// on the id addresses a manager somewhere else, whose sessions are not these.
    pub fn owned_by(&self, payment_manager_arn: &str) -> bool {
        self.payment_manager_arn == payment_manager_arn
    }

    /// The total order payment sessions are reported in: newest first, with the creation
    /// sequence breaking a tie between two sessions created in the same millisecond, so one
    /// state always paginates the same way.
    pub fn order_key(&self) -> (SystemTime, u64) {
        (self.created_at, self.seq)
    }
}

/// Whether the payment manager `payment_manager_arn` addresses is one this emulator holds,
/// read off the component that OWNS payment managers.
///
/// A payment manager is NOT this data plane's resource: it is created, reported and deleted
/// through the AgentCore control plane (`CreatePaymentManager` / `GetPaymentManager` /
/// `DeletePaymentManager`), and nothing this plane serves creates one. So a payment session
/// can only be opened under — and read from — a manager that plane returns from its own
/// read: an ARN addressing none is an ARN addressing nothing, whatever it looks like. This
/// is the idiom the sibling browser plane already uses
/// ([`super::browser::browser_availability`]), for the same reason: a registry entry is not
/// the owner's read, and a manager the owner's store no longer holds is the answer a call
/// has to be refused on.
///
/// The ARN is matched WHOLE, as the owner minted it: it carries the partition, the region
/// and the account a manager lives in, so an ARN that agrees only on the id addresses a
/// manager in some other place.
pub fn payment_manager_availability<Eng: Send + Sync + 'static>(
    engine: &Eng,
    payment_manager_arn: &str,
) -> PaymentManagerAvailability {
    let Some(control) = control_plane::state::service_state(engine) else {
        return PaymentManagerAvailability::StoreUnavailable;
    };
    let store = control.lock();
    if store
        .payment_managers
        .values()
        .any(|held| held.payment_manager_arn == payment_manager_arn)
    {
        PaymentManagerAvailability::Held
    } else {
        PaymentManagerAvailability::Unknown
    }
}

/// What the component that owns payment managers says about one a request named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaymentManagerAvailability {
    /// The account holds it, so a call may go on to the session it addresses.
    Held,
    /// No payment manager of that ARN exists, so nothing can be opened under it or read
    /// from it.
    Unknown,
    /// The control plane's own store could not be reached, which is this emulator's fault
    /// and not the request's.
    StoreUnavailable,
}

/// The pre-condition EVERY call of the payments plane keeps, whether it writes or only
/// reports: the payment manager a request names is one the account holds.
///
/// `Ok(())` when it is. `Err(Some(sentence))` is the denial the plane words a manager it
/// holds none of with ([`PAYMENT_MANAGER_NOT_FOUND_MESSAGE`]); `Err(None)` says the
/// component that owns managers could not be reached, which a handler answers with a
/// modelled server error rather than by denying the caller.
pub fn payment_manager_resolved<Eng: Send + Sync + 'static>(
    engine: &Eng,
    payment_manager_arn: &str,
) -> Result<(), Option<&'static str>> {
    match payment_manager_availability(engine, payment_manager_arn) {
        PaymentManagerAvailability::Held => Ok(()),
        PaymentManagerAvailability::Unknown => Err(Some(PAYMENT_MANAGER_NOT_FOUND_MESSAGE)),
        PaymentManagerAvailability::StoreUnavailable => Err(None),
    }
}

/// Mint a `paymentSessionId` for a session created at `created_at`.
///
/// `PaymentSessionId` is `^payment-session-[0-9a-zA-Z-]{15}$` with length exactly 31, so the
/// id is the prefix the shape fixes followed by fifteen characters: the creation instant in
/// epoch milliseconds (its low 32 bits, eight lowercase hex digits) and the engine-wide
/// sequence (its low 28 bits, seven more), which keeps two sessions opened inside the same
/// millisecond apart.
pub fn mint_payment_session_id(created_at: SystemTime, seq: u64) -> String {
    let millis = created_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_millis());
    format!(
        "{PAYMENT_SESSION_ID_PREFIX}{:08x}{:07x}",
        millis & 0xffff_ffff,
        seq & 0xfff_ffff
    )
}

impl ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState {
    /// The payment session this `CreatePaymentSession` `clientToken` already opened under
    /// `payment_manager_arn`, or `None` when it has opened none this store still holds.
    ///
    /// A token is answered only for the manager it was used under: the same token replayed
    /// against another manager opens that manager's own session.
    pub fn payment_session_for_token(
        &self,
        payment_manager_arn: &str,
        client_token: &str,
    ) -> Option<&PaymentSessionRecord> {
        self.payment_session_idempotency
            .get(client_token)
            .and_then(|session_id| self.payment_sessions.get(session_id))
            .filter(|held| held.owned_by(payment_manager_arn))
    }

    /// Store an opened payment session, recording `client_token` as the token that opened
    /// it, and hold every bound this plane declares.
    pub fn insert_payment_session(
        &mut self,
        record: PaymentSessionRecord,
        client_token: Option<String>,
    ) {
        if let Some(token) = client_token {
            self.payment_session_idempotency
                .insert(token, record.payment_session_id.clone());
        }
        self.payment_sessions
            .insert(record.payment_session_id.clone(), record);

        while self.payment_sessions.len() > MAX_PAYMENT_SESSIONS {
            // The least recently opened session gives way first; the sessions a live
            // workflow just opened are the ones that stay readable.
            let Some(oldest) = self
                .payment_sessions
                .values()
                .min_by_key(|held| held.seq)
                .map(|held| held.payment_session_id.clone())
            else {
                break;
            };
            self.payment_sessions.remove(&oldest);
            // The tokens that pointed at it go with it, as they do on the delete path: a
            // token whose session is gone would otherwise let a replayed create mint a
            // SECOND session instead of answering with the one it already opened.
            self.payment_session_idempotency
                .retain(|_, held| held != &oldest);
        }
        while self.payment_session_idempotency.len() > MAX_PAYMENT_SESSION_IDEMPOTENCY_RECORDS {
            // A token whose session is already gone is of no use to a replay, so it gives
            // way before one that can still be answered.
            let Some(oldest) = self
                .payment_session_idempotency
                .iter()
                .min_by_key(|(token, session_id)| {
                    (
                        self.payment_sessions
                            .get(*session_id)
                            .map_or(0, |held| held.seq),
                        (*token).clone(),
                    )
                })
                .map(|(token, _)| token.clone())
            else {
                break;
            };
            self.payment_session_idempotency.remove(&oldest);
        }
    }

    /// The payment session a read or a delete addressed, or the sentence naming it as the
    /// missing resource ([`payment_session_not_found_message`]).
    ///
    /// A session is addressed WITHIN the manager that owns it, so an id another manager's
    /// session carries is not found here for the same reason one that was never opened is.
    pub fn resolve_payment_session(
        &self,
        payment_manager_arn: &str,
        payment_session_id: &str,
    ) -> Result<&PaymentSessionRecord, String> {
        self.payment_sessions
            .get(payment_session_id)
            .filter(|held| held.owned_by(payment_manager_arn))
            .ok_or_else(|| payment_session_not_found_message(payment_session_id))
    }

    /// The payment sessions `payment_manager_arn` owns, newest first
    /// ([`PaymentSessionRecord::order_key`]), narrowed to the sessions of `user_id` when a
    /// request names one — the member is documented as "the user ID associated with the
    /// payment sessions", so it scopes the listing to that user's own.
    pub fn payment_sessions_newest_first(
        &self,
        payment_manager_arn: &str,
        user_id: Option<&str>,
    ) -> Vec<&PaymentSessionRecord> {
        let mut sessions: Vec<&PaymentSessionRecord> = self
            .payment_sessions
            .values()
            .filter(|held| held.owned_by(payment_manager_arn))
            .filter(|held| user_id.is_none_or(|wanted| held.user_id == wanted))
            .collect();
        sessions.sort_by_key(|held| std::cmp::Reverse(held.order_key()));
        sessions
    }

    /// Remove the payment session a delete addressed and hand it back, or the sentence
    /// naming it as the missing resource.
    ///
    /// The `clientToken` records that point at it go with it: there is nothing left for a
    /// replayed create to answer with.
    pub fn remove_payment_session(
        &mut self,
        payment_manager_arn: &str,
        payment_session_id: &str,
    ) -> Result<PaymentSessionRecord, String> {
        // Resolved first, so a delete naming another manager's session removes nothing.
        self.resolve_payment_session(payment_manager_arn, payment_session_id)?;
        let removed = self
            .payment_sessions
            .remove(payment_session_id)
            .ok_or_else(|| payment_session_not_found_message(payment_session_id))?;
        self.payment_session_idempotency
            .retain(|_, held| held != &removed.payment_session_id);
        Ok(removed)
    }
}
