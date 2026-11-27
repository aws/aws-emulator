// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iamtoolbox-authrequestservice service.
//!
//! The service models only `@readonly` reads of one denied request's authorization
//! details — `GetRequestAuthorizationDetails` and `GetRequestAuthorizationPolicy` — so
//! the store they read from is the whole of this service's state: one record per
//! authorization id, written when a request was denied and read back by the principal
//! that was denied.
//!
//! There is no modeled write operation, so a record enters the store through
//! [`ComAmazonawsIamtoolboxAuthrequestserviceState::record_denial`] — the seam a host
//! embedding this emulator (or a test) uses to record the denial whose details the read
//! then serves. Nothing is seeded at construction: the store starts empty, so a GET for
//! an id this emulator never recorded answers `ResourceNotFoundException`, which is what
//! the real service answers for an id it never issued.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{Evaluation, PolicyInfo};

/// The context keys that apply to a whole request (or to one evaluation): the modeled
/// `AuthorizationContext` map, `map<string, document>`.
pub type AuthorizationContext = HashMap<String, ::aws_smithy_types::Document>;

/// How long a denial's authorization details stay readable.
///
/// The operation's own modeled documentation: "Authorization details are available for
/// at least 24 hours after the denial." 24 hours is therefore the floor the service
/// promises, and the window this emulator keeps a record for; a read of a record older
/// than this answers `ResourceNotFoundException`, the error the model documents for
/// details that "do not exist in this region or have expired".
pub const AUTHORIZATION_DETAILS_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

/// One denied request's authorization details, keyed in the store by its authorization
/// id (the id a supported service puts in the access-denied error message).
#[derive(Debug, Clone)]
pub struct AuthorizationDetailsRecord {
    /// The authorization id this record is keyed by.
    pub authorization_id: String,
    /// Context keys shared by every evaluation of the denied request.
    pub request_context: AuthorizationContext,
    /// The evaluations performed for the denied request, in the order they were
    /// performed; this is the list `GetRequestAuthorizationDetails` paginates.
    pub evaluations: Vec<Evaluation>,
    /// The policies that were evaluated. Request-level, so every page repeats it.
    pub policies: Vec<PolicyInfo>,
    /// The policy document captured with the denied request for each policy URI this
    /// record publishes, keyed by that URI (the `uri` of a [`PolicyInfo`] in
    /// [`policies`](Self::policies)).
    ///
    /// It holds the document as it was CAPTURED at denial time, not the current version
    /// of the policy — which is what `GetRequestAuthorizationPolicy` documents it
    /// returns. A policy the record publishes without a URI (an inline one) is not
    /// addressable, so it has no entry here and cannot be fetched.
    pub policy_documents: HashMap<String, ::aws_smithy_types::Document>,
    /// When the denial happened. Drives the retention window above.
    pub created_at: SystemTime,
}

impl AuthorizationDetailsRecord {
    /// A record of a denial that happened now.
    pub fn new(
        authorization_id: impl Into<String>,
        request_context: AuthorizationContext,
        evaluations: Vec<Evaluation>,
        policies: Vec<PolicyInfo>,
    ) -> Self {
        Self {
            authorization_id: authorization_id.into(),
            request_context,
            evaluations,
            policies,
            policy_documents: HashMap::new(),
            created_at: SystemTime::now(),
        }
    }

    /// The same record, carrying the policy document captured for `policy_uri` — the URI
    /// a caller passes to `GetRequestAuthorizationPolicy` to read that document back.
    #[must_use]
    pub fn with_policy_document(
        mut self,
        policy_uri: impl Into<String>,
        document: ::aws_smithy_types::Document,
    ) -> Self {
        self.policy_documents.insert(policy_uri.into(), document);
        self
    }

    /// The policy document this record captured for `policy_uri`; `None` when the URI is
    /// not one of this record's own policy URIs, so no document was captured for it.
    pub fn policy_document(&self, policy_uri: &str) -> Option<&::aws_smithy_types::Document> {
        self.policy_documents.get(policy_uri)
    }

    /// The same record, as if the denial had happened `age` ago. Lets a caller record a
    /// denial at a known age, which is how the retention window is exercised without
    /// waiting out a day.
    ///
    /// Test support: nothing serving a request calls this, and it is hidden from the
    /// crate's docs so the state API does not advertise a time-travel constructor.
    #[doc(hidden)]
    #[must_use]
    pub fn aged(mut self, age: Duration) -> Self {
        self.created_at = self
            .created_at
            .checked_sub(age)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        self
    }

    /// Whether this record has fallen outside [`AUTHORIZATION_DETAILS_RETENTION`] by
    /// `now`.
    ///
    /// The comparison is strict (`>`), because the documented promise is "available for
    /// AT LEAST 24 hours after the denial": a record whose age is exactly the window is
    /// still inside the promise. A `created_at` in the future (a clock that moved
    /// backwards) is not expired.
    pub fn is_expired_at(&self, now: SystemTime) -> bool {
        now.duration_since(self.created_at)
            .is_ok_and(|age| age > AUTHORIZATION_DETAILS_RETENTION)
    }
}

/// The policy document a captured policy's JSON TEXT is, ready for
/// [`AuthorizationDetailsRecord::with_policy_document`]; `None` for text that is not a
/// JSON document at all.
///
/// A policy store holds a policy as the text it was submitted as, while the modelled
/// `policyDocument` is a `document` — "the structured JSON document of the policy that was
/// captured with the original authorization request", "returned as a JSON object". So the
/// text is parsed ONCE, here, rather than once in every service that records a denial.
pub fn captured_policy_document(policy_text: &str) -> Option<::aws_smithy_types::Document> {
    ::serde_json::from_str::<::serde_json::Value>(policy_text)
        .ok()
        .map(|value| document_of(&value))
}

/// The `Document` a parsed JSON value is: one level of recursion per nesting level of the
/// document above it, which a parsed policy bounds.
fn document_of(value: &::serde_json::Value) -> ::aws_smithy_types::Document {
    use ::aws_smithy_types::{Document, Number};
    match value {
        ::serde_json::Value::Null => Document::Null,
        ::serde_json::Value::Bool(flag) => Document::Bool(*flag),
        ::serde_json::Value::Number(number) => Document::Number(if let Some(n) = number.as_u64() {
            Number::PosInt(n)
        } else if let Some(n) = number.as_i64() {
            Number::NegInt(n)
        } else {
            Number::Float(number.as_f64().unwrap_or_default())
        }),
        ::serde_json::Value::String(text) => Document::String(text.clone()),
        ::serde_json::Value::Array(items) => {
            Document::Array(items.iter().map(document_of).collect())
        }
        ::serde_json::Value::Object(members) => Document::Object(
            members
                .iter()
                .map(|(name, member)| (name.clone(), document_of(member)))
                .collect(),
        ),
    }
}

/// Per-service state for com-amazonaws-iamtoolbox-authrequestservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsIamtoolboxAuthrequestserviceState {
    /// Authorization details by authorization id.
    pub authorization_details: HashMap<String, AuthorizationDetailsRecord>,
}

impl ComAmazonawsIamtoolboxAuthrequestserviceState {
    /// Record the authorization details of a denied request, returning the record the
    /// same authorization id held before, if any. This is the only way a record enters
    /// the store: the service models no write operation.
    ///
    /// Writing also drops every record the clock has already carried past
    /// [`AUTHORIZATION_DETAILS_RETENTION`], so a long-lived store stays bounded by the
    /// retention window rather than by the number of denials ever recorded. The sweep
    /// changes nothing a caller could still have read: an expired record is already
    /// invisible to [`live_record`](Self::live_record).
    pub fn record_denial(
        &mut self,
        record: AuthorizationDetailsRecord,
    ) -> Option<AuthorizationDetailsRecord> {
        let now = SystemTime::now();
        self.authorization_details
            .retain(|_, stored| !stored.is_expired_at(now));
        self.authorization_details
            .insert(record.authorization_id.clone(), record)
    }

    /// The record for `authorization_id` that is still within the retention window at
    /// `now`; `None` when no record was ever stored for that id or the stored one has
    /// expired. Both cases are the same answer on the wire — the details are not
    /// available — so the reader does not distinguish them.
    pub fn live_record(
        &self,
        authorization_id: &str,
        now: SystemTime,
    ) -> Option<&AuthorizationDetailsRecord> {
        self.authorization_details
            .get(authorization_id)
            .filter(|record| !record.is_expired_at(now))
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIamtoolboxAuthrequestserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIamtoolboxAuthrequestserviceState>)
}
