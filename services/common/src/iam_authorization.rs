// SPDX-License-Identifier: CC0-1.0

//! The ONE evaluation of IAM policy documents against a request, for every service that
//! decides server-side whether the caller may perform an action on a resource.
//!
//! An emulated service that enforces authorization has to answer the same question IAM
//! answers: given the documents in force — the calling principal's identity-based
//! policies, and the resource's own resource-based policy — may this principal perform
//! this action on this resource? That is ONE rule, so it lives here once rather than per
//! service: two services asked about the same document must not answer differently. The
//! documents themselves are collected by the service (its own store holds the resource
//! policy, the IAM store holds the principal's), which is why nothing here reaches a
//! store or names a service.
//!
//! The rule, in IAM's own evaluation order ("Determining whether a request is allowed or
//! denied within an account"):
//!
//! * a statement is ABOUT the request only when its action element matches the requested
//!   action, its resource element matches the requested resource, and — for a
//!   resource-based policy — its principal element names the calling principal;
//! * an explicit `Deny` among those statements WINS, whatever any `Allow` says, and
//!   whichever order they are written in;
//! * otherwise the request is allowed when at least one of them is an `Allow`
//!   ([`Decision::Allowed`]); a document that is about the request but says neither is
//!   [`Decision::NotStated`], which within one account means "no grant from here" rather
//!   than "refused" — the identity-based and resource-based decisions are combined by
//!   [`is_allowed`], where either grant suffices and a single deny refuses.
//!
//! A caller that must also report WHICH statements produced the decision — the IAM policy
//! simulator, whose response attributes an allow or a deny to the document it came from —
//! asks through [`identity_outcome_prepared`] / [`resource_outcome_prepared`], which answer
//! that same decision together with the statements it was reached from ([`Outcome`]).
//!
//! How a refusal is WORDED lives here too, for the same reason the decision does:
//! [`not_authorized_by_resource_policy`] is the one sentence an AWS service refuses an
//! unauthorized request with, built from the principal, the action and the resource the
//! request named, and [`not_authorized_to_access_resource`] the one a service states when
//! the caller may not reach the named resource at all, which names the principal alone.
//!
//! `Action`/`NotAction`, `Resource`/`NotResource` and `Principal`/`NotPrincipal` are each
//! either one string or a list of them, and the `Not…` spelling matches everything its
//! values do NOT. IAM compares action names case-insensitively and resource ARNs
//! case-sensitively, with `*` (any run) and `?` (one character) wildcards in both
//! ([`wildcard_match`]); a principal is compared literally, because the IAM grammar gives
//! `Principal` no partial wildcards — only the whole-element `"*"`.
//!
//! ## Why this is not the only pattern matcher in the tree
//!
//! One service carries its own IAM-shaped matcher, and it is not a caller this module can
//! take over: IAM Access Analyzer's `check_no_new_access` owns a POLICY COMPARISON engine
//! (does policy B grant access policy A does not?), which enumerates request contexts
//! under a step and time budget and reports WHY it cannot decide, in that operation's own
//! vocabulary. Its matcher is charged against that budget; this one answers a single
//! access request, where the documents are already bounded by the operations that accepted
//! them, so it needs no budget and reports no reasons.
//!
//! ## Conditions
//!
//! A statement's `Condition` is evaluated only for the keys the caller puts in the
//! [`Request::context`] it passes (`rum:alias`, say, which CloudWatch RUM documents for
//! `PutRumEvents`) and only for the string, ARN, boolean, numeric, date, IP-address and
//! existence operators below, each
//! of which may carry IAM's `IfExists` suffix ([`IF_EXISTS_SUFFIX`]), under which a key the
//! request models and carries no value for holds instead of refusing. A clause this CAN decide is
//! part of whether its statement applies at all, whichever effect the statement states: a
//! `Deny` whose condition demonstrably does not hold refuses nothing. A clause this CANNOT
//! decide — an operator it does not implement, or a key the request context does not carry
//! — makes the statement UNDECIDED, and an undecided statement never grants (an `Allow`
//! whose condition may not hold is not a grant) while it still refuses (a `Deny` whose
//! condition may hold is never weakened into permission). That is the fail-closed reading
//! in both directions; nothing here guesses a condition value.
//!
//! A key the request carries SEVERAL values for — the session tag keys of an STS mint,
//! say — is passed as one `(key, value)` pair per value, and is decided only under one of
//! IAM's set qualifiers (`ForAllValues:` / `ForAnyValue:` before the operator), by IAM's
//! own rule for the empty set: `ForAllValues:` holds when the request carries no value for
//! the key, `ForAnyValue:` does not. That empty-set reading applies only to a key the
//! request DECLARES it models ([`Request::published`]); for every other key, absence from
//! the context means this emulator does not model it and the clause stays undecided, the
//! same fail-closed reading as everywhere else here. Under an UNqualified operator a
//! multi-valued key is left undecided rather than compared against one of its values.
//!
//! IAM's `Null` operator asks about a key's PRESENCE rather than its value, so it is decided
//! under that same declaration and nowhere else: a request that models a key can answer that
//! it carries no value for it, and one that does not model the key cannot. A caller that
//! knows every value of a key FAMILY publishes the family — `aws:PrincipalTag/`, or
//! `kms:EncryptionContext:`, written up to and including the separator the family's members
//! are named after it by — which is what lets a statement conditioned on the absence of one
//! member of that family be decided for a request whose members are all known. A caller
//! that knows the request's WHOLE context — a simulation, whose context is exactly the
//! entries its caller supplied — declares that instead
//! ([`Request::with_complete_context`]), and then every key absent from the context is a
//! key the request carries no value for, whatever its name.
//!
//! ## The keys a decision needed a value for
//!
//! A caller that must report which keys the request should have carried — the IAM policy
//! simulator's `MissingContextValues` — reads them off the decision
//! ([`Outcome::required_context_keys`]) rather than off the documents: a key is reported
//! when a clause reached by the request needed a value for it and the request carried
//! none, so a clause the ABSENCE itself satisfies (a negated operator, an `IfExists`
//! spelling, `Null` asking for absence) needs nothing and names no key.
//!
//! ## Policy variables
//!
//! A policy element may be written with a VARIABLE — `${aws:username}` and the rest of
//! IAM's `${<condition key>}` grammar — which is a placeholder for the value the REQUEST
//! carries for that condition key, substituted at evaluation time and not at write time
//! ("IAM policy elements: variables and tags"). So the document is stored and reported
//! verbatim, and one document decides differently for two callers.
//!
//! Substitution happens here, into a statement's `Resource`/`NotResource` values and into
//! a `Condition` clause's values, out of the same [`Request::context`] the condition
//! operators read — so a key a caller supplies a value for is the key a variable of that
//! name expands to. The substituted text is TEXT: a `*` or `?` that arrives with a value
//! is that character, while one the document itself wrote is still a wildcard, since the
//! pattern is what the document states and not what a request puts into it. `${*}`,
//! `${?}` and `${$}` are IAM's three escapes and stand for the single characters they
//! name, which is what keeps a `$` that opens no variable matched literally.
//!
//! A variable the request carries no single value for cannot be substituted, and an
//! element carrying it therefore matches nothing: the statement is read as UNDECIDED, the
//! same fail-closed reading a condition over an unknown key gets — it grants nothing while
//! a `Deny` carrying it still refuses. That is deliberately stricter than reading such an
//! element as a plain non-match: a refusal is never weakened on a value this evaluation
//! could not establish.

/// The string condition operators this evaluator decides. Everything else leaves the
/// statement undecided (see the module
/// docs). Each of them may be written under a set qualifier; see [`split_set_qualifier`].
const STRING_OPERATORS: &[&str] = &[
    "StringEquals",
    "StringNotEquals",
    "StringEqualsIgnoreCase",
    "StringNotEqualsIgnoreCase",
    "StringLike",
    "StringNotLike",
];

/// IAM's existence operator: "`Null` — check if a condition key is present at the time of
/// authorization", where `true` holds when the request carries NO value for the key and
/// `false` when it carries one.
///
/// It is decidable only for a key the request DECLARES it models ([`Request::published`]):
/// for every other key, absence from the context means this emulator does not model the key
/// rather than that the request carried no value for it, and reading the first as the second
/// would turn an undecided clause into a decided one.
const NULL_OPERATOR: &str = "Null";

/// The ARN condition operators this evaluator decides, which is how a resource policy
/// scopes a grant to the resource a request came FROM (`aws:SourceArn`). Like the string
/// operators, each may be written under a set qualifier; see [`split_set_qualifier`].
///
/// "ArnEquals, ArnLike: case-sensitive matching of the ARN. Each of the six colon-delimited
/// components of the ARN is checked separately and each can include a multi-character match
/// wildcard (*) or a single-character match wildcard (?)" — both spellings compare the same
/// way, so both are the wildcard comparison [`wildcard_match`] performs on an ARN, and the
/// `ArnNot…` spellings are their negations.
///
/// The comparison is performed over the WHOLE ARN, the way this module already matches a
/// statement's `Resource` element, rather than component by component: a `*` therefore spans
/// the `:` separators, so `arn:aws:sns:*` matches every ARN of that service where
/// per-component matching would need a wildcard per component. Every pattern that is exact,
/// or wildcards only within one component, is decided identically either way.
const ARN_OPERATORS: &[&str] = &["ArnEquals", "ArnNotEquals", "ArnLike", "ArnNotLike"];

/// The date condition operators this evaluator decides: "date condition operators let you
/// construct Condition elements that restrict access based on comparing a key to a
/// date/time value", used "with `aws:CurrentTime` key or `aws:EpochTime` key". Like the
/// string operators, each may be written under a set qualifier and with the `IfExists`
/// suffix.
///
/// Both sides are read as INSTANTS ([`instant`]) rather than as text, so the two forms IAM
/// admits for the same instant compare equal: "you must specify date/time values with one
/// of the W3C implementations of the ISO 8601 date formats or in epoch (UNIX) time".
const DATE_OPERATORS: &[&str] = &[
    "DateEquals",
    "DateNotEquals",
    "DateLessThan",
    "DateLessThanEquals",
    "DateGreaterThan",
    "DateGreaterThanEquals",
];

/// The instant `text` names, in seconds since the epoch, or `None` when it names none.
///
/// The two forms a date condition's value takes are the two IAM admits: epoch seconds, and
/// an ISO 8601 date-time, whose calendar conversion comes from the maintained library and
/// whose numeric UTC offset is resolved through the tree's one reader of that offset
/// ([`super::rfc3339::split_utc_offset`]). Text in neither form names no instant, so a
/// comparison against it does not hold.
fn instant(text: &str) -> Option<i64> {
    if let Ok(epoch_seconds) = text.parse::<i64>() {
        return Some(epoch_seconds);
    }
    let (civil, offset_seconds) = super::rfc3339::split_utc_offset(text)?;
    let parsed = ::aws_smithy_types::DateTime::from_str(
        &format!("{civil}Z"),
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    parsed.secs().checked_sub(offset_seconds)
}

/// IAM's boolean operator: "`Bool` — lets you construct Condition elements that restrict
/// access based on comparing a key to `true` or `false`". The two literals are compared
/// case-insensitively, which is how a document may spell them.
const BOOL_OPERATOR: &str = "Bool";

/// The numeric condition operators this evaluator decides: "numeric condition operators let
/// you construct `Condition` elements that restrict access based on comparing a key to an
/// integer or decimal value". Like the string operators, each may be written under a set
/// qualifier and with the `IfExists` suffix.
///
/// Both sides are read as NUMBERS ([`number`]) rather than as text, so the spellings of one
/// figure compare equal (`30`, `30.0`, `3e1`) — which is what a key whose value is a count
/// or a size is compared by, and a document may state either side as a JSON number or as
/// its quoted text.
const NUMERIC_OPERATORS: &[&str] = &[
    "NumericEquals",
    "NumericNotEquals",
    "NumericLessThan",
    "NumericLessThanEquals",
    "NumericGreaterThan",
    "NumericGreaterThanEquals",
];

/// The number `text` names, or `None` when it names none — which is what leaves a numeric
/// comparison against it not holding.
///
/// Read as a binary floating-point value, which carries every integer a condition key of
/// this service tree can name (a count of days, a size in bytes, an age in seconds) exactly;
/// a value that is not finite names no figure to compare, so `inf` and `NaN` name none.
fn number(text: &str) -> Option<f64> {
    text.trim()
        .parse::<f64>()
        .ok()
        .filter(|parsed| parsed.is_finite())
}

/// The IP-address condition operators this evaluator decides: "IP address condition
/// operators let you construct `Condition` elements that restrict access based on comparing
/// a key to an IPv4 or IPv6 address or range of IP addresses", used with a key whose value
/// is an address (`aws:SourceIp`, `aws:VpcSourceIp`). Like the string operators, each may be
/// written under a set qualifier and with the `IfExists` suffix.
///
/// Both sides are read as ADDRESSES rather than as text ([`in_network`]): "IpAddress — the
/// specified IP address or range" holds when the address the request carries lies in the
/// network the clause states, so a clause stating a CIDR block holds for every address of
/// that block and not only for the one spelling of it, and `NotIpAddress` holds exactly
/// where its sibling does not.
const IP_ADDRESS_OPERATORS: &[&str] = &["IpAddress", "NotIpAddress"];

/// Whether the address `actual` names lies in the network `stated` names.
///
/// Both sides are read by the tree's IP-network parser rather than by comparing text, so the
/// comparison is the one the network describes: a clause stating `198.51.100.0/24` holds for
/// every address of that block, and one stating a bare address holds for that address alone
/// — "you can use a /32 prefix size to specify a single IP address", which is the network a
/// bare address is. A side that names no address, and an address of the other IP version
/// than the network's, lies in nothing, so the positive comparison does not hold on it (and
/// the negated one, which holds exactly where its sibling does not, does).
fn in_network(stated: &str, actual: &str) -> bool {
    let Ok(address) = actual.trim().parse::<::std::net::IpAddr>() else {
        return false;
    };
    let stated = stated.trim();
    match stated.parse::<::ipnet::IpNet>() {
        Ok(network) => network.contains(&address),
        // A clause may state a single address rather than a block, which is that address's
        // own host network.
        Err(_) => stated
            .parse::<::std::net::IpAddr>()
            .is_ok_and(|single| single == address),
    }
}

/// The suffix that makes a comparison hold where the request carries no value for the key:
/// "you can add `IfExists` to the end of any condition operator name except the `Null`
/// condition … if the condition key is present in the request context, process the key as
/// specified in the policy; if the key is not present, evaluate the condition element as
/// true".
///
/// Absence is only decidable for a key the request DECLARES it models
/// ([`Request::published`]); for every other key the clause stays undecided, because there
/// absence means this emulator does not model the key rather than that the request carries
/// no value for it.
const IF_EXISTS_SUFFIX: &str = "IfExists";

/// The characters a condition-key FAMILY's members are named after it by, which is what a
/// published name ending in one declares the whole family of: IAM's tag families name a
/// member after a `/` (`aws:PrincipalTag/team`) and a service's per-entry families name one
/// after a `:` (`kms:EncryptionContext:tenant`).
const FAMILY_SEPARATORS: [char; 2] = ['/', ':'];

/// The most bytes one policy document this evaluator reads may carry.
///
/// Every document that reaches here came through an operation that bounds it (IAM bounds a
/// managed policy at 6,144 characters and an inline role policy at 10,240; CloudWatch RUM
/// bounds an app monitor's resource policy at 4 KB), so this is a backstop an accepted
/// document never meets: it keeps a document that somehow got past those from making the
/// wildcard matcher — whose cost is the product of the pattern and value lengths — do
/// unbounded work. A longer document is read as stating nothing.
pub const MAX_POLICY_BYTES: usize = 64 * 1024;

/// What one set of policy documents says about a request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Decision {
    /// At least one statement about the request is an `Allow`, and none is a `Deny`.
    Allowed,
    /// A statement about the request is an explicit `Deny`.
    Denied,
    /// No statement is about the request at all (or the only ones that are cannot be
    /// decided): these documents neither grant nor refuse.
    #[default]
    NotStated,
}

/// One statement that CONTRIBUTED to a decision, as WHERE it was read from.
///
/// Nothing here names a policy: which document of the set is which policy is the
/// caller's own knowledge — it passed them — so a caller reporting the attribution
/// names the documents in its own vocabulary rather than in this module's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchedStatement {
    /// Which document of the set the decision was made from, counted from zero in the
    /// order the caller passed them.
    pub document: usize,
    /// Which statement of that document's `Statement` element, counted from zero in
    /// document order.
    pub statement: usize,
}

/// What a set of documents decided, together with the statements that decided it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// The decision itself: the same answer [`identity_decision_prepared`] gives for the
    /// same documents and request.
    pub decision: Decision,
    /// The statements the decision was reached from, in document order: the DENYING ones
    /// when it is [`Decision::Denied`] — "even if multiple statements allow the operation
    /// on the resource, if only one statement denies that operation, then the explicit
    /// deny overrides any allow. In addition, the deny statement is the only entry
    /// included in the result" — the ALLOWING ones when it is [`Decision::Allowed`], and
    /// none when the documents state nothing about the request.
    pub matched: Vec<MatchedStatement>,
    /// The condition keys the statements ABOUT this request needed a value for and the
    /// request carried none, folded to one entry per key.
    ///
    /// A clause the absence of its key already satisfies needs no value for it — a
    /// negated operator, an `IfExists` spelling and a `Null` asking for absence all hold
    /// on a key the request does not carry — so such a key is not named here; a key whose
    /// clause this evaluator could not decide at all is. Nothing but a `Condition` names a
    /// key: the keys a statement's other elements interpolate as policy variables are not
    /// values the request was required to carry.
    pub required_context_keys: Vec<String>,
}

/// The access request a decision is made about.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    /// The IAM action, service prefix included: `rum:PutRumEvents`.
    pub action: &'a str,
    /// The ARN of the resource the action is taken on.
    pub resource: &'a str,
    /// Every spelling of the calling principal a resource-based policy may name — the
    /// assumed-role session ARN and the role's own ARN, say. Unused when the documents are
    /// identity-based, where statements name no principal.
    pub principals: &'a [String],
    /// The condition keys whose values this request knows, as `(key, value)` pairs. A
    /// clause over any other key leaves its statement undecided.
    pub context: &'a [(String, String)],
    /// The condition keys this request MODELS, whether or not it carries a value for
    /// each — the keys whose absence from [`Self::context`] means "the caller passed
    /// none" rather than "this emulator does not model the key". Only these get IAM's
    /// empty-set reading under a set qualifier; see the module docs.
    pub published: &'a [&'a str],
    /// Whether [`Self::context`] is the request's WHOLE condition context, so that EVERY
    /// key absent from it is a key the request carries no value for — which is what a
    /// caller holding the whole context declares (see the module docs), and what makes
    /// every key published rather than only the named ones.
    pub context_complete: bool,
}

impl<'a> Request<'a> {
    /// A request with no condition context: the common case, where the operation
    /// publishes no condition keys of its own.
    pub fn new(action: &'a str, resource: &'a str, principals: &'a [String]) -> Self {
        Self {
            action,
            resource,
            principals,
            context: &[],
            published: &[],
            context_complete: false,
        }
    }

    /// The same request, carrying the condition keys `context` names.
    pub fn with_context(mut self, context: &'a [(String, String)]) -> Self {
        self.context = context;
        self
    }

    /// The same request, declaring the condition keys it MODELS. See
    /// [`Self::published`].
    #[must_use]
    pub fn with_published_keys(mut self, published: &'a [&'a str]) -> Self {
        self.published = published;
        self
    }

    /// The same request, declaring that the context it carries is the WHOLE context: every
    /// key absent from it is one the request carries no value for. See
    /// [`Self::context_complete`].
    #[must_use]
    pub fn with_complete_context(mut self) -> Self {
        self.context_complete = true;
        self
    }
}

/// Which grammar a document follows, which is what decides how its statements are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flavor {
    /// Attached to a principal, so a statement names no `Principal` and must name the
    /// resources it covers.
    Identity,
    /// Attached to the resource, so a statement names the principals it grants to, and a
    /// statement that names no `Resource` covers the resource the policy is attached to.
    Resource,
}

/// What the principal's identity-based policies say about the request.
///
/// `documents` are the policies in force on the caller — its inline policies and the
/// documents of the managed policies attached to it — in any order: the result does not
/// depend on it, because a `Deny` in any of them wins.
///
/// Each document is READ here, so a caller asking about SEVERAL requests over the same
/// documents (one action on each of the resources it reaches, say) reads them once with
/// [`prepare_documents`] and asks through [`identity_decision_prepared`] instead, which
/// keeps the cost of n requests n statement walks rather than n parses.
pub fn identity_decision(documents: &[String], request: &Request<'_>) -> Decision {
    identity_decision_prepared(&prepare_documents(documents), request)
}

/// One policy document READ ONCE, for a caller that decides several requests against the
/// same documents.
///
/// A document this cannot read as a policy — one past [`MAX_POLICY_BYTES`], or one that is
/// not a policy at all — is held as "states nothing", which is exactly what deciding it
/// would have answered; the operation that accepted the document is what refuses a
/// malformed one.
#[derive(Debug, Clone)]
pub struct PreparedDocument {
    policy: Option<::serde_json::Value>,
}

impl PreparedDocument {
    /// Read `document` once.
    #[must_use]
    pub fn read(document: &str) -> Self {
        let policy = (document.len() <= MAX_POLICY_BYTES)
            .then(|| super::iam_trust_policy::parse_policy(document))
            .flatten();
        Self { policy }
    }
}

/// Every document of `documents`, read once each. See [`PreparedDocument`].
#[must_use]
pub fn prepare_documents(documents: &[String]) -> Vec<PreparedDocument> {
    documents
        .iter()
        .map(|document| PreparedDocument::read(document))
        .collect()
}

/// [`identity_decision`] over documents already read: the SAME rule, and the same answer
/// for the same documents — the reading is all that is hoisted out.
///
/// A caller that asks only for the DECISION collects no
/// [`Outcome::required_context_keys`]: that report is work the caller does not read, and
/// this is the entry point a service's request-time gate takes while it holds its store's
/// lock.
#[must_use]
pub fn identity_decision_prepared(
    documents: &[PreparedDocument],
    request: &Request<'_>,
) -> Decision {
    outcome(
        documents,
        request,
        Flavor::Identity,
        RequiredKeys::unreported(),
    )
    .decision
}

/// What the resource's own resource-based policy says about the request.
///
/// The decision alone, so nothing is collected for a report (see
/// [`identity_decision_prepared`]).
pub fn resource_decision(document: &str, request: &Request<'_>) -> Decision {
    outcome(
        ::std::slice::from_ref(&PreparedDocument::read(document)),
        request,
        Flavor::Resource,
        RequiredKeys::unreported(),
    )
    .decision
}

/// [`identity_decision_prepared`], with the statements the decision was reached from and
/// the keys it needed a value for.
///
/// For a caller that must also report WHICH documents produced the decision — the IAM
/// policy simulator attributes an allow or a deny to the policy it came from — rather
/// than only what the decision is. The decision is the SAME one
/// [`identity_decision_prepared`] gives: both come from this one walk.
#[must_use]
pub fn identity_outcome_prepared(
    documents: &[PreparedDocument],
    request: &Request<'_>,
) -> Outcome {
    outcome(documents, request, Flavor::Identity, RequiredKeys::reported())
}

/// [`resource_decision`] over a document already read, with the statements the decision
/// was reached from. The one document is document `0` of the outcome.
#[must_use]
pub fn resource_outcome_prepared(document: &PreparedDocument, request: &Request<'_>) -> Outcome {
    outcome(
        ::std::slice::from_ref(document),
        request,
        Flavor::Resource,
        RequiredKeys::reported(),
    )
}

/// Whether the request is allowed, given the two decisions.
///
/// IAM's order: an explicit `Deny` from either side refuses; otherwise, within one
/// account, an `Allow` from EITHER the identity-based policies or the resource-based
/// policy is enough ("the resource-based policy and the identity-based policy are
/// evaluated together, and access is granted if either allows it"). With neither — the
/// implicit deny — the request is refused.
pub fn is_allowed(identity: Decision, resource: Decision) -> bool {
    if identity == Decision::Denied || resource == Decision::Denied {
        return false;
    }
    identity == Decision::Allowed || resource == Decision::Allowed
}

/// Whether these identity-based documents could allow `action` AT ALL — on SOME resource,
/// whichever one it is.
///
/// A request is granted only by an `Allow` whose action element covers the requested action
/// AND whose resource element covers the requested resource, so documents holding no `Allow`
/// of the action are about no request for it, whatever resource it names: every such request
/// falls to the default deny. That is the question a caller asks here when it knows the
/// ACTION but not the resource ARNs the request addresses — a service's request-time gate
/// standing in front of an action that supports resource-level permissions, where
/// withholding a grant on [`identity_decision`] over `*` alone would refuse a request a
/// resource-scoped statement permits.
///
/// Deliberately the WEAKER question, in the same direction the rest of this module reads an
/// undecidable clause: `true` for a grant scoped to resources this request may not even be
/// about, and `true` for an `Allow` whose `Condition` this evaluator cannot decide, so a
/// caller that refuses on `false` refuses only what no readable document can permit. It
/// answers nothing about `Deny`: an explicit deny is decided against the resource, through
/// [`identity_decision`].
#[must_use]
pub fn identity_may_allow_action(documents: &[PreparedDocument], action: &str) -> bool {
    documents.iter().any(|document| {
        statements(document).into_iter().any(|statement| {
            statement["Effect"]
                .as_str()
                .unwrap_or_default()
                .eq_ignore_ascii_case("Allow")
                && names_action(statement, action)
        })
    })
}

/// The spellings a policy's `Principal`/`NotPrincipal` element names a whole ACCOUNT by:
/// that account's root ARN and its bare account id.
///
/// `arn` is any ARN of a principal in the account — the account and the partition are read
/// out of it through the tree's one ARN parse ([`super::arn::parse`]), so a principal in
/// another partition is answered in that partition. An ARN this cannot read, or one
/// carrying no account, names no account and is answered with no spellings rather than a
/// guess.
///
/// Naming the ACCOUNT is not naming a principal, which is why the two sets are separate:
/// IAM reads a resource statement that ALLOWS an account as delegating the decision to
/// that account's own identity-based policies ("the default key policy … gives the Amazon
/// Web Services account that owns the KMS key permission to use it"), while one that
/// DENIES the account refuses each of its principals. So a caller decides a DENY with
/// these spellings beside the principal's own, and a GRANT with the principal's alone.
///
/// `service-com-amazonaws-sqs-amazonsqs/src/state/authorization.rs` and
/// `…/src/state/encryption.rs` each still carry their own spelling of the account root,
/// inlined before this one existed and pinned by that service's suites; they are to be
/// migrated onto this rather than copied again.
#[must_use]
pub fn account_principals(arn: &str) -> Vec<String> {
    let Some(parsed) = super::arn::parse(arn) else {
        return Vec::new();
    };
    if parsed.account.is_empty() {
        return Vec::new();
    }
    vec![
        format!("arn:{}:iam::{}:root", parsed.partition, parsed.account),
        parsed.account.to_string(),
    ]
}

/// The sentence AWS refuses a request with when the RESOURCE it names grants the calling
/// principal nothing: the principal that signed, the action it asked for and the resource
/// it asked for, each as the request carried them.
///
/// `action` is the fully qualified IAM action (`<service action namespace>:<ApiName>`), so
/// the one sentence serves every service: the namespace is the caller's to supply, because
/// only the service knows the namespace its own actions are published under. Nothing about
/// the resource is looked up to state it — an authorization refusal is decided from the
/// request alone, which is why it can be stated before the resource is resolved.
///
/// Several services inlined this sentence before it lived here —
/// `grep -rln 'because no resource-based policy allows' services/*/src` lists them — and
/// they are to be migrated onto this one rather than copied again.
#[must_use]
pub fn not_authorized_by_resource_policy(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         because no resource-based policy allows the {action} action"
    )
}

/// The sentence AWS refuses a request with when the CALLING PRINCIPAL's own identity-based
/// documents do not allow it: the principal that signed, the action it asked for, the
/// resource it asked for, and `reason` — which of IAM's rules withheld the grant.
///
/// The counterpart of [`not_authorized_by_resource_policy`] for the identity-based half of
/// the decision, and stated here for the same reason: the sentence is one rule, so two
/// services that reach the same decision must word it the same way. The reasons a decision
/// can rest on are the ones this module names beside it ([`IDENTITY_POLICY_EXPLICIT_DENY`],
/// [`no_identity_policy_allows`], [`SESSION_POLICY_EXPLICIT_DENY`],
/// [`no_session_policy_allows`], [`PERMISSIONS_BOUNDARY_EXPLICIT_DENY`],
/// [`no_permissions_boundary_allows`]), so a caller states none of them itself. Every value comes from the request and from the stored
/// policies, and nothing about the resource is looked up — which is why the refusal can be
/// stated before the resource is resolved.
///
/// `action` is the fully qualified IAM action (`<service action namespace>:<ApiName>`): the
/// namespace is the caller's to supply, because only the service knows the namespace its own
/// actions are published under. `resource` is the ARN the request addressed; a call that
/// addresses no single resource of its own is outside this sentence, since there is nothing
/// to name in it.
///
/// Every gate that decides the identity-based question inlined this sentence before it lived
/// here — `grep -rln 'because no identity-based policy allows' services/*/src` lists them —
/// and they are to be migrated onto this one rather than copied again.
#[must_use]
pub fn not_authorized_by_identity_policy(
    principal: &str,
    action: &str,
    resource: &str,
    reason: &str,
) -> String {
    format!(
        "{} {reason}",
        not_authorized_to_perform(principal, action, resource)
    )
}

/// The same sentence WITHOUT a reason clause: the principal that signed, the action it asked
/// for and the resource it asked for, and nothing further.
///
/// It is [`not_authorized_by_identity_policy`] minus the clause, and the two are stated in
/// one place for that reason. Some services' recorded refusal carries no clause naming which
/// of IAM's rules withheld the grant, so ONE sentence answers an explicit `Deny` and IAM's
/// default deny alike; a gate reaches for this form only where a recording of its own
/// service's refusal shows it, and for every other gate
/// [`not_authorized_by_identity_policy`] with the matching clause
/// ([`IDENTITY_POLICY_EXPLICIT_DENY`], [`no_identity_policy_allows`]) is the sentence.
///
/// `action` is the fully qualified IAM action and `resource` what the request addressed, as
/// in [`not_authorized_by_identity_policy`]; both come from the request, so this too can be
/// stated before the resource is resolved.
#[must_use]
pub fn not_authorized_to_perform(principal: &str, action: &str, resource: &str) -> String {
    format!("User: {principal} is not authorized to perform: {action} on resource: {resource}")
}

/// The reason clause for a request an explicit `Deny` in one of the principal's own
/// identity-based documents refuses: a fact about that document, so the sentence names it
/// rather than the action.
pub const IDENTITY_POLICY_EXPLICIT_DENY: &str = "with an explicit deny in an identity-based policy";

/// The reason clause for a request an explicit `Deny` in the principal's PERMISSIONS BOUNDARY
/// refuses.
pub const PERMISSIONS_BOUNDARY_EXPLICIT_DENY: &str =
    "with an explicit deny in a permissions boundary";

/// The reason clause for a request an explicit `Deny` in one of the SESSION POLICIES the
/// caller's temporary credential was minted with refuses — a fact about that document, so the
/// sentence names it rather than the action.
pub const SESSION_POLICY_EXPLICIT_DENY: &str = "with an explicit deny in a session policy";

/// The reason clause for a request no readable identity-based document allows — IAM's default
/// deny, which names the action nothing granted.
#[must_use]
pub fn no_identity_policy_allows(action: &str) -> String {
    format!("because no identity-based policy allows the {action} action")
}

/// The reason clause for a grant the principal's permissions boundary does not also allow:
/// "a permissions boundary sets the maximum permissions that an identity-based policy can
/// grant" (IAM, permissions boundaries).
#[must_use]
pub fn no_permissions_boundary_allows(action: &str) -> String {
    format!("because no permissions boundary allows the {action} action")
}

/// The reason clause for a grant the SESSION POLICIES the caller's temporary credential was
/// minted with do not also allow: "the resulting session's permissions are the intersection
/// of the role's identity-based policy and the session policies"
/// (`AssumeRoleRequest$PolicyArns`), so a scope that states nothing about the action withholds
/// it exactly as a permissions boundary that states nothing does.
#[must_use]
pub fn no_session_policy_allows(action: &str) -> String {
    format!("because no session policy allows the {action} action")
}

/// The sentence AWS refuses a request with when the caller may not reach the named resource
/// AT ALL: the calling principal, and nothing else.
///
/// It is the denial of the services whose refusal is decided from the request's own resource
/// ARN rather than from a policy evaluation — a resource field naming another account, which
/// no document of this account can grant over — so it names neither an action nor a
/// resource: a caller must not be able to learn from a denial what exists in an account that
/// is not its own. That is also why it can be stated before anything is looked up.
///
/// `service-com-amazonaws-snowdevicemanagement-snowdevicemanagement/src/state.rs` and
/// `service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/src/service/authorization.rs`
/// each still carry their own spelling of this sentence, inlined before it lived here and
/// pinned by their own suites; they are to be migrated onto this one rather than copied
/// again.
#[must_use]
pub fn not_authorized_to_access_resource(principal: &str) -> String {
    format!("User: {principal} is not authorized to access this resource")
}

/// Whether `pattern` matches `value`, with IAM's `*` (any run of characters, including
/// none) and `?` (exactly one character) wildcards.
///
/// `case_sensitive` is false for an action name (IAM compares those case-insensitively)
/// and true for a resource ARN. The walk is the classic single-pass wildcard match with
/// one backtrack point per `*`, so its cost is bounded by the product of the two lengths
/// and it never recurses.
///
/// Every `*` and `?` of `pattern` is a wildcard here, which is what an element the
/// document wrote with no policy variable in it means; an element that names one is
/// matched through [`Expanded`], where the characters a variable's VALUE contributed are
/// the characters themselves.
pub fn wildcard_match(pattern: &str, value: &str, case_sensitive: bool) -> bool {
    Expanded::literal(pattern).matches(value, case_sensitive)
}

/// How IAM opens a policy variable, closed by the next `}`.
const VARIABLE_OPEN: &str = "${";

/// IAM's three literal escapes, each standing for the one character it names rather than
/// for a variable or a wildcard: "`${*}`, `${?}`, `${$}` … use these special characters in
/// a policy variable to specify a literal `*`, `?` or `$` character".
const VARIABLE_ESCAPES: [&str; 3] = ["*", "?", "$"];

/// What a policy element's text means once its policy VARIABLES are substituted: the
/// characters to match, each carrying whether it is still one of IAM's wildcards.
///
/// A `*` or `?` the DOCUMENT wrote is a wildcard; one that arrived as part of a
/// substituted value is the character itself, because a variable's value is text the
/// request supplied rather than a pattern the document stated.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Expanded {
    glyphs: Vec<(char, bool)>,
}

impl Expanded {
    /// `text` read as a pattern in its own right: every `*` and `?` in it is a wildcard,
    /// which is what an element carrying no policy variable states.
    fn literal(text: &str) -> Self {
        let mut glyphs = Vec::with_capacity(text.len());
        push_pattern(&mut glyphs, text);
        Self { glyphs }
    }

    /// Whether this element's characters ARE `actual`, which is what an equality
    /// comparison asks: a wildcard is special only to a pattern match. Compared without
    /// building the text, so a clause decided against many values allocates nothing per
    /// comparison.
    fn equals(&self, actual: &str, case_sensitive: bool) -> bool {
        let mut glyphs = self.glyphs.iter().map(|(glyph, _)| *glyph);
        let mut actual = actual.chars();
        loop {
            match (glyphs.next(), actual.next()) {
                (None, None) => return true,
                (Some(expected), Some(reached))
                    if expected == reached
                        || (!case_sensitive && expected.eq_ignore_ascii_case(&reached)) => {}
                _ => return false,
            }
        }
    }

    /// This element's characters as TEXT, wildcards included as the characters they are.
    /// What a comparison that is not a text match at all reads the element through — a
    /// date clause, whose two sides are instants.
    fn text(&self) -> String {
        self.glyphs.iter().map(|(glyph, _)| *glyph).collect()
    }

    /// Whether this element matches `value`, by [`wildcard_match`]'s walk over the
    /// characters that are still wildcards.
    fn matches(&self, value: &str, case_sensitive: bool) -> bool {
        if case_sensitive {
            return walk(&self.glyphs, &value.chars().collect::<Vec<char>>());
        }
        // IAM folds case on the comparisons that are case-insensitive, which is the one
        // reason either side is rebuilt.
        let pattern: Vec<(char, bool)> = self
            .glyphs
            .iter()
            .flat_map(|(glyph, wildcard)| {
                let wildcard = *wildcard;
                glyph.to_lowercase().map(move |folded| (folded, wildcard))
            })
            .collect();
        walk(&pattern, &value.to_lowercase().chars().collect::<Vec<char>>())
    }
}

/// Whether `pattern` matches `value`: the single-pass wildcard walk, with one backtrack
/// point per `*`, over the characters of `pattern` that are still wildcards.
fn walk(pattern: &[(char, bool)], value: &[char]) -> bool {
    let mut pattern_at = 0;
    let mut value_at = 0;
    // The last `*` seen, and how much of `value` it had consumed when it was seen: the
    // one place the walk returns to when a literal comparison fails after it.
    let mut star_at: Option<usize> = None;
    let mut star_consumed = 0;
    while value_at < value.len() {
        match pattern.get(pattern_at) {
            Some(('*', true)) => {
                star_at = Some(pattern_at);
                star_consumed = value_at;
                pattern_at += 1;
            }
            Some(('?', true)) => {
                pattern_at += 1;
                value_at += 1;
            }
            Some((literal, _)) if *literal == value[value_at] => {
                pattern_at += 1;
                value_at += 1;
            }
            // Either the pattern ran out or it disagrees here: give the last `*` one
            // more character and try again from just after it.
            _ => match star_at {
                Some(star) => {
                    pattern_at = star + 1;
                    star_consumed += 1;
                    value_at = star_consumed;
                }
                None => return false,
            },
        }
    }
    // Trailing `*`s match the empty remainder.
    pattern[pattern_at..]
        .iter()
        .all(|(glyph, wildcard)| *glyph == '*' && *wildcard)
}

/// Push `text` onto `glyphs` as PATTERN text: the characters it spells, with its `*` and
/// `?` marked as the wildcards they are.
fn push_pattern(glyphs: &mut Vec<(char, bool)>, text: &str) {
    glyphs.extend(
        text.chars()
            .map(|glyph| (glyph, glyph == '*' || glyph == '?')),
    );
}

/// `value` with every policy variable it names substituted from the request's condition
/// context, or `None` when one of them has no single value there — an element this
/// evaluation cannot match at all (see the module docs).
///
/// The text around a variable keeps its wildcards; the text a variable contributes does
/// not acquire any. A `${` that is never closed opens no variable, so what follows it is
/// matched as the pattern text it is.
fn expand(value: &str, request: &Request<'_>) -> Option<Expanded> {
    if !value.contains(VARIABLE_OPEN) {
        return Some(Expanded::literal(value));
    }
    let mut glyphs: Vec<(char, bool)> = Vec::with_capacity(value.len());
    let mut rest = value;
    while let Some(at) = rest.find(VARIABLE_OPEN) {
        push_pattern(&mut glyphs, &rest[..at]);
        let opened = &rest[at + VARIABLE_OPEN.len()..];
        let Some(end) = opened.find('}') else {
            push_pattern(&mut glyphs, &rest[at..]);
            return Some(Expanded { glyphs });
        };
        let name = &opened[..end];
        rest = &opened[end + 1..];
        if VARIABLE_ESCAPES.contains(&name) {
            // The one character the escape names, which is never a wildcard and never a
            // variable opener.
            glyphs.extend(name.chars().map(|glyph| (glyph, false)));
            continue;
        }
        // The key's value as text: a `*` or `?` inside it is that character.
        glyphs.extend(
            single_value(request, name)?
                .chars()
                .map(|glyph| (glyph, false)),
        );
        // Substitution is the one thing that makes an element LONGER than the document it
        // was read from, so the bound every document is already held to is taken over the
        // expanded element too: one past it is read as matching nothing, exactly as a
        // document past [`MAX_POLICY_BYTES`] is read as stating nothing. The matcher's
        // cost is the product of the pattern's length and the value's, which is what this
        // keeps bounded however many variables an element names.
        if glyphs.len() > MAX_POLICY_BYTES {
            return None;
        }
    }
    push_pattern(&mut glyphs, rest);
    Some(Expanded { glyphs })
}

/// The ONE value the request's context carries for `key`, compared the same
/// case-insensitive way a condition clause's key is.
///
/// `None` for a key the context carries no value for — nothing to substitute — and for one
/// it carries several, since a multi-valued key names no single text a variable could
/// stand for.
fn single_value<'a>(request: &Request<'a>, key: &str) -> Option<&'a str> {
    let mut values = request
        .context
        .iter()
        .filter(|(known, _)| known.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.as_str());
    let only = values.next()?;
    values.next().is_none().then_some(only)
}

/// What one SET of documents decides about the request, and which of their statements
/// decided it.
///
/// The ONE walk every answer this module gives comes from, so a caller reading the
/// attribution can never be told a decision the attribution does not support: a `Deny`
/// among the statements about the request wins over every `Allow`, whichever document and
/// whichever order they are written in, and is then the only thing the decision came from.
fn outcome(
    documents: &[PreparedDocument],
    request: &Request<'_>,
    flavor: Flavor,
    mut required: RequiredKeys,
) -> Outcome {
    let mut applying: Vec<(bool, MatchedStatement)> = Vec::new();
    for (position, document) in documents.iter().enumerate() {
        collect_applying(document, request, flavor, position, &mut applying, &mut required);
    }
    let denied = applying.iter().any(|(denies, _)| *denies);
    let decision = match (denied, applying.is_empty()) {
        (true, _) => Decision::Denied,
        (false, false) => Decision::Allowed,
        (false, true) => Decision::NotStated,
    };
    Outcome {
        decision,
        matched: applying
            .into_iter()
            .filter(|(denies, _)| *denies == denied)
            .map(|(_, matched)| matched)
            .collect(),
        required_context_keys: required.into_reported(),
    }
}

/// How many condition keys ONE decision reports as needed but unsupplied
/// ([`Outcome::required_context_keys`]).
///
/// The keys come from the documents the caller passed, whose number and size the caller
/// bounds, so the report is bounded here rather than growing with what a document carries:
/// past this many the rest are not collected at all. It is deliberately well above the
/// bound a caller reports its own list under (the IAM policy simulator refuses a request
/// whose documents need more than a hundred), so such a caller can still see that its own
/// bound was passed rather than being handed a list truncated to it.
pub const MAX_REQUIRED_CONTEXT_KEYS: usize = 1_000;

/// The keys one decision needed a value for and the request carried none, collected while
/// the statements are walked.
///
/// Folded by case, the way a condition key's name is compared everywhere here, so one key
/// is one entry — held in a map, so noting a key costs a lookup rather than a scan of what
/// was noted before — and stopped at [`MAX_REQUIRED_CONTEXT_KEYS`].
///
/// A caller that does not report the list collects NOTHING ([`RequiredKeys::unreported`]):
/// the request-time gate only needs the decision, so it allocates nothing for this on the
/// path that holds a service's store lock.
#[derive(Debug)]
struct RequiredKeys {
    by_fold: ::std::collections::BTreeMap<String, String>,
    /// How many keys this collection keeps; `0` collects none at all.
    cap: usize,
}

impl RequiredKeys {
    /// A collection for a caller that reports the keys, bounded by
    /// [`MAX_REQUIRED_CONTEXT_KEYS`].
    fn reported() -> Self {
        Self {
            by_fold: ::std::collections::BTreeMap::new(),
            cap: MAX_REQUIRED_CONTEXT_KEYS,
        }
    }

    /// A collection for a caller that does not report them, which keeps none.
    fn unreported() -> Self {
        Self {
            by_fold: ::std::collections::BTreeMap::new(),
            cap: 0,
        }
    }

    /// Note `key` as one the clause reached needed a value for, when the request carried
    /// none (`actual` empty) and the bound has not been reached.
    fn note(&mut self, key: &str, actual: &[&str]) {
        if !actual.is_empty() || self.by_fold.len() >= self.cap {
            return;
        }
        self.by_fold
            .entry(key.to_ascii_lowercase())
            .or_insert_with(|| key.to_string());
    }

    /// The keys to report: one entry per key, in a total order, each in the spelling the
    /// document that named it first used.
    fn into_reported(self) -> Vec<String> {
        self.by_fold.into_values().collect()
    }
}

/// The statements of ONE document, in document order: the `Statement` element's entries,
/// or the single statement a document that states one object carries.
///
/// Empty for a document this could not read as a policy at all, and for one whose
/// `Statement` is neither — both state nothing, which is what walking no statement answers.
fn statements(document: &PreparedDocument) -> Vec<&::serde_json::Value> {
    let Some(policy) = document.policy.as_ref() else {
        return Vec::new();
    };
    let statement = &policy["Statement"];
    match statement {
        ::serde_json::Value::Array(entries) => entries.iter().collect(),
        ::serde_json::Value::Object(_) => vec![statement],
        _ => Vec::new(),
    }
}

/// Every statement of ONE document that APPLIES to the request, pushed onto `into` in
/// document order as `(whether it denies, where it was read from)`.
///
/// `required` collects the condition keys the statements about the request needed a value
/// for and the request carried none ([`Outcome::required_context_keys`]).
fn collect_applying(
    document: &PreparedDocument,
    request: &Request<'_>,
    flavor: Flavor,
    position: usize,
    into: &mut Vec<(bool, MatchedStatement)>,
    required: &mut RequiredKeys,
) {
    for (at, entry) in statements(document).into_iter().enumerate() {
        let effect = entry["Effect"].as_str().unwrap_or_default();
        let denies = effect.eq_ignore_ascii_case("Deny");
        let allows = effect.eq_ignore_ascii_case("Allow");
        if !denies && !allows {
            // Neither of the two effects the grammar admits: the statement says nothing.
            continue;
        }
        // Whether the statement is about the request at all. An element naming a policy
        // variable the request has no value for cannot be matched, which leaves this
        // undecided — read fail-closed in both directions, exactly as an undecidable
        // condition clause is below.
        match covers(entry, request, flavor) {
            Some(false) => continue,
            None if allows => continue,
            _ => {}
        }
        // The statement's condition, where this evaluator can decide it, is part of
        // whether the statement applies at all — to a `Deny` as much as to an `Allow`. A
        // clause it CANNOT decide is read fail-closed in both directions: it withholds a
        // grant and keeps a refusal (see the module docs).
        match conditions_hold(entry, request, required) {
            // Decidably false: the statement does not apply, whichever effect it states.
            Some(false) => continue,
            // Undecidable: no grant, but a refusal stands.
            None if allows => continue,
            _ => {}
        }
        into.push((
            denies,
            MatchedStatement {
                document: position,
                statement: at,
            },
        ));
    }
}

/// Whether one statement is ABOUT the request — whatever effect it then states.
///
/// `None` when that cannot be decided: a `Resource` element naming a policy variable the
/// request carries no value for (see the module docs).
fn covers(
    statement: &::serde_json::Value,
    request: &Request<'_>,
    flavor: Flavor,
) -> Option<bool> {
    if !names_action(statement, request.action)
        || !names_principal(statement, request.principals, flavor)
    {
        return Some(false);
    }
    names_resource(statement, request, flavor)
}

/// Whether the statement's `Action`/`NotAction` element covers the requested action.
fn names_action(statement: &::serde_json::Value, action: &str) -> bool {
    match element(statement, "Action", "NotAction") {
        Some((values, negated)) => {
            let hit = values
                .iter()
                .any(|pattern| wildcard_match(pattern, action, false));
            hit != negated
        }
        // A statement that names no action at all governs nothing.
        None => false,
    }
}

/// Whether the statement's `Resource`/`NotResource` element covers the requested resource.
///
/// Each value is read with its policy variables substituted ([`expand`]), so one document
/// scopes a grant per caller. `None` when a value names a variable the request carries no
/// value for AND no other value settles the element: that element matches nothing this
/// evaluation can establish, which is neither a match nor a miss.
fn names_resource(
    statement: &::serde_json::Value,
    request: &Request<'_>,
    flavor: Flavor,
) -> Option<bool> {
    match element(statement, "Resource", "NotResource") {
        Some((values, negated)) => {
            let mut hit = false;
            let mut unexpanded = false;
            for value in values {
                match expand(value, request) {
                    Some(pattern) => hit = hit || pattern.matches(request.resource, true),
                    None => unexpanded = true,
                }
            }
            // A value that DOES match settles the element whichever spelling it is: the
            // resource is among the values (so `Resource` covers it and `NotResource` does
            // not), whatever an unsubstitutable sibling value would have said.
            if hit {
                return Some(!negated);
            }
            (!unexpanded).then_some(negated)
        }
        // An identity-based statement must name the resources it covers. A
        // resource-based one is attached to the resource, so leaving the element out
        // covers exactly that resource.
        None => Some(flavor == Flavor::Resource),
    }
}

/// Whether the statement's `Principal`/`NotPrincipal` element names the caller.
fn names_principal(
    statement: &::serde_json::Value,
    principals: &[String],
    flavor: Flavor,
) -> bool {
    let named = principal_values(&statement["Principal"]);
    let not_named = principal_values(&statement["NotPrincipal"]);
    match flavor {
        // An identity-based policy's statements name no principal: the policy is attached
        // to one. A statement that DOES name one is a resource-policy statement, and is
        // not read as a grant to its own holder.
        Flavor::Identity => named.is_none() && not_named.is_none(),
        Flavor::Resource => {
            let (values, negated) = match (named, not_named) {
                (Some(values), _) => (values, false),
                (None, Some(values)) => (values, true),
                // A resource-based statement that names no principal grants to nobody.
                (None, None) => return false,
            };
            let hit = values.iter().any(|value| {
                *value == "*" || principals.iter().any(|principal| principal == value)
            });
            hit != negated
        }
    }
}

/// The values of a policy element, and whether they came from its negated spelling.
///
/// `None` when the statement carries neither spelling. The positive spelling wins when a
/// statement carries both, which is not a document IAM accepts at all.
fn element<'a>(
    statement: &'a ::serde_json::Value,
    positive: &str,
    negative: &str,
) -> Option<(Vec<&'a str>, bool)> {
    let named = super::iam_trust_policy::json_strings(&statement[positive]);
    if !named.is_empty() {
        return Some((named, false));
    }
    let not_named = super::iam_trust_policy::json_strings(&statement[negative]);
    if !not_named.is_empty() {
        return Some((not_named, true));
    }
    None
}

/// The principals a `Principal`/`NotPrincipal` element names, flattened.
///
/// The element is either the string `"*"`, or a map keyed by principal kind (`AWS`,
/// `Service`, `Federated`, `CanonicalUser`) whose values are each one string or a list of
/// them. `None` when the statement does not carry the element.
fn principal_values(value: &::serde_json::Value) -> Option<Vec<&str>> {
    match value {
        ::serde_json::Value::String(one) => Some(vec![one.as_str()]),
        ::serde_json::Value::Array(many) => Some(
            many.iter()
                .filter_map(::serde_json::Value::as_str)
                .collect(),
        ),
        ::serde_json::Value::Object(kinds) => Some(
            kinds
                .values()
                .flat_map(super::iam_trust_policy::json_strings)
                .collect(),
        ),
        _ => None,
    }
}

/// Whether every condition clause this statement carries holds for the request.
///
/// `Some(true)` when the statement carries no `Condition` or every clause holds,
/// `Some(false)` when a clause this evaluator decides does NOT hold (so the statement does
/// not apply, whichever effect it states), and `None` when a clause cannot be decided at
/// all — an operator outside the operator tables, or a single-valued
/// key the request context does not carry — which the caller reads fail-closed in both
/// directions.
///
/// `required` collects the keys a clause here needed a value for and the request carried
/// none ([`RequiredKeys::note`]), whether or not the clause could be decided without one.
///
/// A key the request MODELS and carries no value for is a key that really is not present,
/// which is the case IAM's `IfExists` suffix ([`IF_EXISTS_SUFFIX`]) is about: under it such
/// a clause HOLDS, so a grant written `…IfExists` applies to a request that carries no value
/// for the key while the same grant without the suffix does not.
///
/// A key the request carries SEVERAL values for (the session tag keys of an STS mint,
/// say) is decided only under one of the set qualifiers, and then by IAM's own rule for
/// the empty set: `ForAllValues:` "returns true if there are no request values" while
/// `ForAnyValue:` returns false. That empty-set reading applies ONLY to a key the request
/// DECLARES it models ([`Request::published`]), because here absence from the context
/// otherwise means "this emulator does not model the key" rather than "the caller passed
/// no values", and reading the first as the second would turn an undecided `Allow` into a
/// grant. A multi-valued key under an UNqualified operator is left undecided rather than
/// compared against one of its values.
fn conditions_hold(
    statement: &::serde_json::Value,
    request: &Request<'_>,
    required: &mut RequiredKeys,
) -> Option<bool> {
    let Some(clauses) = statement["Condition"].as_object() else {
        // No condition at all holds trivially; an element that is not a map is not a
        // condition this reads, so it cannot be decided.
        return statement["Condition"].is_null().then_some(true);
    };
    let mut held = true;
    // Whether every clause of this statement could be decided at all. The walk goes on
    // after one that could not, so the keys the LATER clauses needed a value for are
    // reported too; the answer is the same `None` an immediate stop would have given.
    let mut decidable = true;
    for (clause, keys) in clauses {
        let Some(keys) = keys.as_object() else {
            decidable = false;
            continue;
        };
        let (qualifier, named) = split_set_qualifier(clause);
        let (operator, absence_holds) = split_if_exists(named);
        let existence = operator == NULL_OPERATOR;
        // The suffix is defined for every operator BUT the existence one, so `NullIfExists`
        // names no operator at all and decides nothing.
        let implemented = if existence {
            !absence_holds
        } else {
            operator == BOOL_OPERATOR
                || STRING_OPERATORS.contains(&operator)
                || ARN_OPERATORS.contains(&operator)
                || NUMERIC_OPERATORS.contains(&operator)
                || DATE_OPERATORS.contains(&operator)
                || IP_ADDRESS_OPERATORS.contains(&operator)
        };
        for (key, expected) in keys {
            let actual: Vec<&str> = request
                .context
                .iter()
                .filter(|(known, _)| known.eq_ignore_ascii_case(key))
                .map(|(_, value)| value.as_str())
                .collect();
            let expected = clause_values(expected);
            // Whether the request MODELS this key at all, which is what makes an absent
            // value mean "the request carries none" rather than "this emulator does not
            // know".
            let modelled = publishes(request, key);
            if !implemented {
                required.note(key, &actual);
                decidable = false;
                continue;
            }
            if existence {
                // `Null` asks about the key's PRESENCE, so a value is never compared: the
                // clause holds when the request's answer agrees with what it asserts.
                let asserted_absent = match (modelled, &expected[..]) {
                    (true, [one]) => one.eq_ignore_ascii_case("true"),
                    _ => {
                        required.note(key, &actual);
                        decidable = false;
                        continue;
                    }
                };
                let holds = asserted_absent == actual.is_empty();
                if !holds {
                    required.note(key, &actual);
                }
                held = held && holds;
                continue;
            }
            // A clause's own VALUES may name policy variables, substituted from this same
            // context the way a `Resource` element's are. A value naming a variable the
            // request carries no value for cannot be compared at all, which leaves the
            // statement undecided.
            let Some(expected) = expected
                .iter()
                .map(|value| expand(value, request))
                .collect::<Option<Vec<Expanded>>>()
            else {
                required.note(key, &actual);
                decidable = false;
                continue;
            };
            let decided = match qualifier {
                // Unqualified: the key must carry exactly one value for the comparison
                // to mean what IAM means by it.
                None => match actual[..] {
                    [one] => clause_holds(operator, &expected, one),
                    // A key the request MODELS and carries no value for is one the request
                    // really does not carry, which IAM compares nothing against: the
                    // `IfExists` spelling holds on it, "if the key is not present … a
                    // negated condition operator returns true", and a positive one without
                    // the suffix does not hold.
                    [] if modelled => absence_holds || is_negated(operator),
                    _ => {
                        required.note(key, &actual);
                        decidable = false;
                        continue;
                    }
                },
                // Under a qualifier, a key with no values is IAM's empty set — but only
                // where the request DECLARES it models the key; otherwise its absence is
                // this emulator not modelling it, which decides nothing.
                Some(_) if actual.is_empty() && !modelled => {
                    required.note(key, &actual);
                    decidable = false;
                    continue;
                }
                // The suffix answers absence before the set rule does, so a key the request
                // carries no value for holds under either qualifier.
                Some(_) if actual.is_empty() && absence_holds => true,
                Some(SetQualifier::ForAllValues) => actual
                    .iter()
                    .all(|value| clause_holds(operator, &expected, value)),
                Some(SetQualifier::ForAnyValue) => actual
                    .iter()
                    .any(|value| clause_holds(operator, &expected, value)),
            };
            if !decided {
                required.note(key, &actual);
            }
            held = held && decided;
        }
    }
    decidable.then_some(held)
}

/// The values one condition clause states for a key: the strings it names, and the TEXT of
/// a scalar it states unquoted.
///
/// IAM's policy grammar writes a condition value as a string, and a document may equally
/// state a numeric or a boolean one as the JSON number or boolean that names it ("Bool":
/// {"aws:SecureTransport": true}) — the same value, so the two spellings decide alike. A
/// composite value (an object, a list of lists) names none.
fn clause_values(value: &::serde_json::Value) -> Vec<String> {
    match value {
        ::serde_json::Value::Array(many) => many.iter().filter_map(scalar_text).collect(),
        one => scalar_text(one).into_iter().collect(),
    }
}

/// The text a scalar JSON value names, or `None` for a value that is not one.
fn scalar_text(value: &::serde_json::Value) -> Option<String> {
    match value {
        ::serde_json::Value::String(text) => Some(text.clone()),
        ::serde_json::Value::Number(number) => Some(number.to_string()),
        ::serde_json::Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// Whether the request DECLARES that it models `key`: its own name is published, or a
/// published FAMILY covers it.
///
/// A published name ending in the separator its members are named after it by is a whole
/// key family — `aws:PrincipalTag/` covers `aws:PrincipalTag/team` and every other tag key,
/// `kms:EncryptionContext:` covers every entry name of a request's encryption context —
/// which is what a caller that knows ALL the values of a family publishes: it can answer
/// for a key of that family whether the request carries a value for it, including answering
/// that it carries none. A request whose context is COMPLETE
/// ([`Request::context_complete`]) can answer that for every key there is, so it publishes
/// them all.
fn publishes(request: &Request<'_>, key: &str) -> bool {
    request.context_complete
        || request.published.iter().any(|known| {
            known.eq_ignore_ascii_case(key)
                || (known.ends_with(FAMILY_SEPARATORS)
                    && key.len() > known.len()
                    // By `get`, never by a byte-index slice: the key is read verbatim out
                    // of a policy document, so a multi-byte character straddling the
                    // family's length must answer "not covered" rather than panic.
                    && key
                        .get(..known.len())
                        .is_some_and(|head| head.eq_ignore_ascii_case(known)))
        })
}

/// The set qualifier a condition clause's key may be written with, for a request key that
/// carries SEVERAL values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetQualifier {
    /// Every value the request carries for the key must satisfy the clause.
    ForAllValues,
    /// At least one value the request carries for the key must satisfy it.
    ForAnyValue,
}

/// Split a condition clause's name into its set qualifier, if it carries one, and the
/// operator itself: `ForAllValues:StringEquals` is `StringEquals` under
/// [`SetQualifier::ForAllValues`].
fn split_set_qualifier(clause: &str) -> (Option<SetQualifier>, &str) {
    for (prefix, qualifier) in [
        ("ForAllValues:", SetQualifier::ForAllValues),
        ("ForAnyValue:", SetQualifier::ForAnyValue),
    ] {
        if let Some(operator) = clause.strip_prefix(prefix) {
            return (Some(qualifier), operator);
        }
    }
    (None, clause)
}

/// Split a condition operator into the comparison it performs and whether a key the request
/// carries no value for holds under it: `ArnEqualsIfExists` is `ArnEquals` where absence
/// holds. See [`IF_EXISTS_SUFFIX`].
fn split_if_exists(operator: &str) -> (&str, bool) {
    match operator.strip_suffix(IF_EXISTS_SUFFIX) {
        Some(comparison) => (comparison, true),
        None => (operator, false),
    }
}

/// Whether an operator is a NEGATED one — the `…Not…` spelling of a comparison, which holds
/// exactly where its positive sibling does not. It is the one thing a clause over a key the
/// request carries NO value for can still be decided on.
fn is_negated(operator: &str) -> bool {
    operator.contains("Not")
}

/// Whether one decided clause holds: `expected` are the clause's values, with their policy
/// variables already substituted, and `actual` the value the request carries for its key.
fn clause_holds(operator: &str, expected: &[Expanded], actual: &str) -> bool {
    let any_equals = |case_sensitive: bool| {
        expected
            .iter()
            .any(|value| value.equals(actual, case_sensitive))
    };
    let any_like = || expected.iter().any(|pattern| pattern.matches(actual, true));
    // Both sides as ADDRESSES: an IP clause asks whether the address the request carries
    // lies in the network the clause states, which is decided by the network rather than by
    // the text of either side.
    let any_in_network = || {
        expected
            .iter()
            .any(|value| in_network(&value.text(), actual))
    };
    // Both sides as INSTANTS: a date clause compares the moment each side names, so the
    // epoch and ISO 8601 spellings of one moment compare equal. A side that names no
    // instant at all is compared against nothing, so the positive spelling does not hold
    // on it — and the negated one, which holds exactly where its sibling does not, does.
    // Both sides as NUMBERS: a numeric clause compares the figure each side names, so the
    // spellings of one figure compare equal. A side that names no figure at all is compared
    // against nothing, so the positive spelling does not hold on it — and the negated one,
    // which holds exactly where its sibling does not, does.
    let any_number = |ordered: fn(f64, f64) -> bool| {
        number(actual).is_some_and(|reached| {
            expected
                .iter()
                .any(|value| number(&value.text()).is_some_and(|stated| ordered(reached, stated)))
        })
    };
    let any_instant = |ordered: fn(i64, i64) -> bool| {
        instant(actual).is_some_and(|reached| {
            expected
                .iter()
                .any(|value| instant(&value.text()).is_some_and(|stated| ordered(reached, stated)))
        })
    };
    match operator {
        "StringEquals" => any_equals(true),
        "StringNotEquals" => !any_equals(true),
        "StringEqualsIgnoreCase" => any_equals(false),
        "StringNotEqualsIgnoreCase" => !any_equals(false),
        "StringLike" => any_like(),
        "StringNotLike" => !any_like(),
        // An ARN is compared case-sensitively with the same two wildcards, and the two
        // positive spellings are documented as doing the same comparison.
        "ArnEquals" | "ArnLike" => any_like(),
        "ArnNotEquals" | "ArnNotLike" => !any_like(),
        "NumericEquals" => any_number(|reached, stated| reached == stated),
        "NumericNotEquals" => !any_number(|reached, stated| reached == stated),
        "NumericLessThan" => any_number(|reached, stated| reached < stated),
        "NumericLessThanEquals" => any_number(|reached, stated| reached <= stated),
        "NumericGreaterThan" => any_number(|reached, stated| reached > stated),
        "NumericGreaterThanEquals" => any_number(|reached, stated| reached >= stated),
        "DateEquals" => any_instant(|reached, stated| reached == stated),
        "DateNotEquals" => !any_instant(|reached, stated| reached == stated),
        "DateLessThan" => any_instant(|reached, stated| reached < stated),
        "DateLessThanEquals" => any_instant(|reached, stated| reached <= stated),
        "DateGreaterThan" => any_instant(|reached, stated| reached > stated),
        "DateGreaterThanEquals" => any_instant(|reached, stated| reached >= stated),
        // An address key is compared against the network the clause states, not against its
        // spelling ([`in_network`]).
        "IpAddress" => any_in_network(),
        "NotIpAddress" => !any_in_network(),
        // A boolean key carries one of the two literals, compared as such.
        BOOL_OPERATOR => any_equals(false),
        // Unreachable: the caller checks the operator against the two operator tables
        // first.
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The principal the rows below call with, in the two spellings a resource policy may
    /// name it by.
    fn principals() -> Vec<String> {
        vec![
            "arn:aws:sts::000000000000:assumed-role/writer/session".to_string(),
            "arn:aws:iam::000000000000:role/writer".to_string(),
        ]
    }

    /// The resource every row below asks about.
    const RESOURCE: &str = "arn:aws:rum:us-east-1:000000000000:appmonitor/shop";

    /// One identity-based document with the statement text given.
    fn identity(statement: &str) -> String {
        format!("{{\"Version\":\"2012-10-17\",\"Statement\":[{statement}]}}")
    }

    #[test]
    fn an_identity_policy_grants_exactly_the_action_and_resource_it_names() {
        let rows: [(&str, &str, Decision); 10] = [
            (
                "the action and the resource, exactly",
                "{\"Effect\":\"Allow\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"arn:aws:rum:us-east-1:000000000000:appmonitor/shop\"}",
                Decision::Allowed,
            ),
            (
                "a service wildcard and every resource",
                "{\"Effect\":\"Allow\",\"Action\":\"rum:*\",\"Resource\":\"*\"}",
                Decision::Allowed,
            ),
            (
                "a prefix wildcard in the action",
                "{\"Effect\":\"Allow\",\"Action\":\"rum:Put*\",\"Resource\":\"*\"}",
                Decision::Allowed,
            ),
            (
                "a single-character wildcard in the resource",
                "{\"Effect\":\"Allow\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"arn:aws:rum:us-east-1:000000000000:appmonitor/sho?\"}",
                Decision::Allowed,
            ),
            (
                "the action in another casing, which IAM folds",
                "{\"Effect\":\"Allow\",\"Action\":\"RUM:PUTRUMEVENTS\",\"Resource\":\"*\"}",
                Decision::Allowed,
            ),
            (
                "another action",
                "{\"Effect\":\"Allow\",\"Action\":\"rum:GetAppMonitor\",\"Resource\":\"*\"}",
                Decision::NotStated,
            ),
            (
                "another resource",
                "{\"Effect\":\"Allow\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"arn:aws:rum:us-east-1:000000000000:appmonitor/other\"}",
                Decision::NotStated,
            ),
            (
                "an explicit deny",
                "{\"Effect\":\"Deny\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\"}",
                Decision::Denied,
            ),
            (
                "a NotAction that excludes the action",
                "{\"Effect\":\"Allow\",\"NotAction\":\"rum:PutRumEvents\",\"Resource\":\"*\"}",
                Decision::NotStated,
            ),
            (
                "a statement naming no resource, which an identity policy must",
                "{\"Effect\":\"Allow\",\"Action\":\"rum:PutRumEvents\"}",
                Decision::NotStated,
            ),
        ];

        let held = principals();
        for (case, statement, expected) in rows {
            let request = Request::new("rum:PutRumEvents", RESOURCE, &held);
            let reached = identity_decision(&[identity(statement)], &request);
            assert_eq!(
                reached, expected,
                "case {case:?}: expected {expected:?}, got {reached:?}"
            );
        }
    }

    #[test]
    fn a_resource_policy_grants_only_the_principals_it_names() {
        let rows: [(&str, &str, Decision); 7] = [
            (
                "the session principal",
                "{\"Effect\":\"Allow\",\"Principal\":{\"AWS\":\"arn:aws:sts::000000000000:assumed-role/writer/session\"},\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\"}",
                Decision::Allowed,
            ),
            (
                "the role principal",
                "{\"Effect\":\"Allow\",\"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:role/writer\"},\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\"}",
                Decision::Allowed,
            ),
            (
                "every principal",
                "{\"Effect\":\"Allow\",\"Principal\":\"*\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\"}",
                Decision::Allowed,
            ),
            (
                "a statement naming no resource, which the attached resource answers",
                "{\"Effect\":\"Allow\",\"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:role/writer\"},\"Action\":\"rum:PutRumEvents\"}",
                Decision::Allowed,
            ),
            (
                "another principal",
                "{\"Effect\":\"Allow\",\"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:role/reader\"},\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\"}",
                Decision::NotStated,
            ),
            (
                "the account root, which delegates to the identity policies",
                "{\"Effect\":\"Allow\",\"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"},\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\"}",
                Decision::NotStated,
            ),
            (
                "a deny naming the principal",
                "{\"Effect\":\"Deny\",\"Principal\":\"*\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\"}",
                Decision::Denied,
            ),
        ];

        let held = principals();
        for (case, statement, expected) in rows {
            let request = Request::new("rum:PutRumEvents", RESOURCE, &held);
            let reached = resource_decision(&identity(statement), &request);
            assert_eq!(
                reached, expected,
                "case {case:?}: expected {expected:?}, got {reached:?}"
            );
        }
    }

    #[test]
    fn an_explicit_deny_wins_whichever_document_and_order_it_is_in() {
        let allow =
            "{\"Effect\":\"Allow\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\"}".to_string();
        let deny =
            "{\"Effect\":\"Deny\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\"}".to_string();
        let held = principals();
        let request = Request::new("rum:PutRumEvents", RESOURCE, &held);
        for (case, documents) in [
            (
                "allow then deny, in one document",
                vec![format!("{{\"Statement\":[{allow},{deny}]}}")],
            ),
            (
                "deny then allow, in one document",
                vec![format!("{{\"Statement\":[{deny},{allow}]}}")],
            ),
            (
                "allow in one document, deny in another",
                vec![identity(&allow), identity(&deny)],
            ),
        ] {
            let reached = identity_decision(&documents, &request);
            assert_eq!(
                reached,
                Decision::Denied,
                "case {case:?}: an explicit deny wins, got {reached:?}"
            );
        }
        // And the combination rule: a deny on either side refuses, an allow on either
        // side suffices.
        let rows: [(&str, Decision, Decision, bool); 6] = [
            ("identity allows", Decision::Allowed, Decision::NotStated, true),
            ("the resource allows", Decision::NotStated, Decision::Allowed, true),
            ("both allow", Decision::Allowed, Decision::Allowed, true),
            ("neither states anything", Decision::NotStated, Decision::NotStated, false),
            ("identity denies what the resource allows", Decision::Denied, Decision::Allowed, false),
            ("the resource denies what identity allows", Decision::Allowed, Decision::Denied, false),
        ];
        for (case, identity_said, resource_said, expected) in rows {
            let allowed = is_allowed(identity_said, resource_said);
            assert_eq!(
                allowed, expected,
                "case {case:?}: expected allowed={expected}, got {allowed}"
            );
        }
    }

    #[test]
    fn a_condition_is_honoured_only_where_the_request_carries_its_key() {
        let statement = |condition: &str| {
            identity(&format!(
                "{{\"Effect\":\"Allow\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\",\
                 \"Condition\":{condition}}}"
            ))
        };
        let held = principals();
        let context = [("rum:alias".to_string(), "shop-web".to_string())];
        let rows: [(&str, &str, Decision); 6] = [
            (
                "StringEquals on a key the request carries, matching",
                "{\"StringEquals\":{\"rum:alias\":\"shop-web\"}}",
                Decision::Allowed,
            ),
            (
                "StringEquals on a key the request carries, not matching",
                "{\"StringEquals\":{\"rum:alias\":\"other\"}}",
                Decision::NotStated,
            ),
            (
                "StringLike with a wildcard",
                "{\"StringLike\":{\"rum:alias\":\"shop-*\"}}",
                Decision::Allowed,
            ),
            (
                "StringNotEquals on another value",
                "{\"StringNotEquals\":{\"rum:alias\":\"other\"}}",
                Decision::Allowed,
            ),
            (
                "a key the request does not carry",
                "{\"StringEquals\":{\"aws:SourceVpc\":\"vpc-1\"}}",
                Decision::NotStated,
            ),
            (
                "an operator this evaluator does not decide",
                "{\"DateLessThan\":{\"aws:CurrentTime\":\"2020-01-01T00:00:00Z\"}}",
                Decision::NotStated,
            ),
        ];
        for (case, condition, expected) in rows {
            let request =
                Request::new("rum:PutRumEvents", RESOURCE, &held).with_context(&context);
            let reached = identity_decision(&[statement(condition)], &request);
            assert_eq!(
                reached, expected,
                "case {case:?}: expected {expected:?}, got {reached:?}"
            );
        }
        // A conditional DENY is never weakened by a condition this cannot decide — and a
        // conditional DENY this CAN decide refuses only where its condition holds.
        let request = Request::new("rum:PutRumEvents", RESOURCE, &held);
        let denial = identity(
            "{\"Effect\":\"Deny\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\",\
             \"Condition\":{\"DateLessThan\":{\"aws:CurrentTime\":\"2020-01-01T00:00:00Z\"}}}",
        );
        let reached = identity_decision(&[denial], &request);
        assert_eq!(
            reached,
            Decision::Denied,
            "an undecidable condition must not turn a Deny into permission, got {reached:?}"
        );

        let deny_rows: [(&str, &str, Decision); 2] = [
            (
                "a decidable Deny condition that holds",
                "{\"StringEquals\":{\"rum:alias\":\"shop-web\"}}",
                Decision::Denied,
            ),
            (
                "a decidable Deny condition that does not hold",
                "{\"StringEquals\":{\"rum:alias\":\"other\"}}",
                Decision::NotStated,
            ),
        ];
        for (case, condition, expected) in deny_rows {
            let document = identity(&format!(
                "{{\"Effect\":\"Deny\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\",\
                 \"Condition\":{condition}}}"
            ));
            let request =
                Request::new("rum:PutRumEvents", RESOURCE, &held).with_context(&context);
            let reached = identity_decision(&[document], &request);
            assert_eq!(
                reached, expected,
                "case {case:?}: expected {expected:?}, got {reached:?}"
            );
        }
    }

    /// The ARN operators decide a clause over an ARN-valued key the same way their string
    /// counterparts decide one over a string: that is what lets a resource policy scope a
    /// grant to the resource a cross-service request came from.
    #[test]
    fn an_arn_condition_is_decided_the_way_its_string_counterpart_is() {
        let source = "arn:aws:sns:us-east-1:000000000000:alarms";
        let context = [("aws:SourceArn".to_string(), source.to_string())];
        let held = principals();
        // (case, the `Condition` element, the decision it must reach)
        let rows: [(&str, &str, Decision); 8] = [
            (
                "ArnEquals on the very ARN the request carries",
                "{\"ArnEquals\":{\"aws:SourceArn\":\"arn:aws:sns:us-east-1:000000000000:alarms\"}}",
                Decision::Allowed,
            ),
            (
                "ArnEquals on another ARN",
                "{\"ArnEquals\":{\"aws:SourceArn\":\"arn:aws:sns:us-east-1:000000000000:other\"}}",
                Decision::NotStated,
            ),
            (
                "ArnLike with a wildcard that matches",
                "{\"ArnLike\":{\"aws:SourceArn\":\"arn:aws:sns:us-east-1:000000000000:*\"}}",
                Decision::Allowed,
            ),
            (
                "ArnLike with a wildcard that does not match",
                "{\"ArnLike\":{\"aws:SourceArn\":\"arn:aws:sqs:*\"}}",
                Decision::NotStated,
            ),
            (
                "ArnNotEquals on another ARN",
                "{\"ArnNotEquals\":{\"aws:SourceArn\":\"arn:aws:sns:us-east-1:000000000000:other\"}}",
                Decision::Allowed,
            ),
            (
                "ArnNotLike on the ARN the request carries",
                "{\"ArnNotLike\":{\"aws:SourceArn\":\"arn:aws:sns:us-east-1:000000000000:*\"}}",
                Decision::NotStated,
            ),
            (
                "an ARN clause over a key the request does not carry",
                "{\"ArnEquals\":{\"aws:SourceAccount\":\"000000000000\"}}",
                Decision::NotStated,
            ),
            (
                "an ARN comparison is case-sensitive, like a resource ARN",
                "{\"ArnEquals\":{\"aws:SourceArn\":\"ARN:AWS:SNS:US-EAST-1:000000000000:ALARMS\"}}",
                Decision::NotStated,
            ),
        ];
        for (case, condition, expected) in rows {
            let document = identity(&format!(
                "{{\"Effect\":\"Allow\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\",\
                 \"Condition\":{condition}}}"
            ));
            let request =
                Request::new("rum:PutRumEvents", RESOURCE, &held).with_context(&context);
            let reached = identity_decision(&[document], &request);
            assert_eq!(
                reached, expected,
                "case {case:?}: expected {expected:?}, got {reached:?}"
            );
        }
    }

    /// RULE: the numeric operators compare the FIGURE each side names, so a clause over a
    /// count- or size-valued key holds exactly where the comparison it writes holds of the
    /// value the request carries, in either side's spelling of that figure.
    #[test]
    fn a_numeric_condition_compares_the_figure_each_side_names() {
        // (case, the value the request carries for the key, the `Condition` element, the
        // decision it must reach). The rows differ along what the rule generalises over:
        // the comparison, which side of the bound the value is, the boundary itself, the
        // spelling of each figure, and a value that names no figure at all.
        let rows: [(&str, &str, &str, Decision); 12] = [
            (
                "NumericEquals on the very figure the request carries",
                "30",
                "{\"NumericEquals\":{\"svc:count\":\"30\"}}",
                Decision::Allowed,
            ),
            (
                "NumericEquals on another figure",
                "30",
                "{\"NumericEquals\":{\"svc:count\":\"7\"}}",
                Decision::NotStated,
            ),
            (
                "NumericNotEquals on another figure",
                "30",
                "{\"NumericNotEquals\":{\"svc:count\":\"7\"}}",
                Decision::Allowed,
            ),
            (
                "NumericLessThan, with the value below the bound",
                "7",
                "{\"NumericLessThan\":{\"svc:count\":\"30\"}}",
                Decision::Allowed,
            ),
            (
                "NumericLessThan, with the value AT the bound",
                "30",
                "{\"NumericLessThan\":{\"svc:count\":\"30\"}}",
                Decision::NotStated,
            ),
            (
                "NumericLessThanEquals, with the value at the bound",
                "30",
                "{\"NumericLessThanEquals\":{\"svc:count\":\"30\"}}",
                Decision::Allowed,
            ),
            (
                "NumericGreaterThan, with the value above the bound",
                "31",
                "{\"NumericGreaterThan\":{\"svc:count\":\"30\"}}",
                Decision::Allowed,
            ),
            (
                "NumericGreaterThanEquals, with the value at the bound",
                "30",
                "{\"NumericGreaterThanEquals\":{\"svc:count\":\"30\"}}",
                Decision::Allowed,
            ),
            (
                "a decimal bound the integer value is under",
                "30",
                "{\"NumericLessThan\":{\"svc:count\":\"30.5\"}}",
                Decision::Allowed,
            ),
            (
                "the same figure spelled as a JSON number rather than as text",
                "30",
                "{\"NumericEquals\":{\"svc:count\":30}}",
                Decision::Allowed,
            ),
            (
                "the same figure in another spelling on each side",
                "30.0",
                "{\"NumericEquals\":{\"svc:count\":\"3e1\"}}",
                Decision::Allowed,
            ),
            (
                "a value that names no figure at all",
                "thirty",
                "{\"NumericLessThan\":{\"svc:count\":\"30\"}}",
                Decision::NotStated,
            ),
        ];
        let held = principals();
        for (case, carried, condition, expected) in rows {
            let context = [("svc:count".to_string(), carried.to_string())];
            let document = identity(&format!(
                "{{\"Effect\":\"Allow\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\",\
                 \"Condition\":{condition}}}"
            ));
            let request = Request::new("rum:PutRumEvents", RESOURCE, &held).with_context(&context);
            let reached = identity_decision(&[document], &request);
            assert_eq!(
                reached, expected,
                "case {case:?}: expected {expected:?}, got {reached:?}"
            );
        }
    }

    /// IAM's `IfExists` suffix: a clause over a key the request really does not carry HOLDS
    /// under it, where the same comparison without the suffix grants nothing — which is what
    /// lets a resource policy state "scope this grant to the resource it came from, if it
    /// came from one".
    #[test]
    fn the_if_exists_suffix_holds_where_the_request_carries_no_value_for_the_key() {
        let source = "arn:aws:sns:us-east-1:000000000000:alarms";
        let carried = [("aws:SourceArn".to_string(), source.to_string())];
        let none: [(String, String); 0] = [];
        let held = principals();
        // The keys the request DECLARES it models, so absence from the context is the
        // request carrying no value rather than this evaluator not knowing the key.
        let published = ["aws:SourceArn", "aws:SecureTransport"];
        // The rows differ along what the rule generalises over: the operator family (ARN,
        // string, boolean, existence), whether it is negated, whether the suffix is written
        // at all, whether the key is carried, modelled-but-absent or not modelled, and
        // whether a set qualifier is written before it.
        type SuffixRow<'a> = (&'a str, &'a str, &'a [(String, String)], Decision);
        let rows: [SuffixRow<'_>; 10] = [
            (
                "ArnEqualsIfExists on the very ARN the request carries",
                "{\"ArnEqualsIfExists\":{\"aws:SourceArn\":\"arn:aws:sns:us-east-1:000000000000:alarms\"}}",
                &carried,
                Decision::Allowed,
            ),
            (
                "ArnEqualsIfExists on another ARN, with the key carried",
                "{\"ArnEqualsIfExists\":{\"aws:SourceArn\":\"arn:aws:sns:us-east-1:000000000000:other\"}}",
                &carried,
                Decision::NotStated,
            ),
            (
                "ArnEqualsIfExists on a modelled key the request carries no value for",
                "{\"ArnEqualsIfExists\":{\"aws:SourceArn\":\"arn:aws:sns:us-east-1:000000000000:alarms\"}}",
                &none,
                Decision::Allowed,
            ),
            (
                "the same clause WITHOUT the suffix, which compares nothing and grants nothing",
                "{\"ArnEquals\":{\"aws:SourceArn\":\"arn:aws:sns:us-east-1:000000000000:alarms\"}}",
                &none,
                Decision::NotStated,
            ),
            (
                "StringEqualsIfExists on a modelled key the request carries no value for",
                "{\"StringEqualsIfExists\":{\"aws:SourceArn\":\"arn:aws:sns:us-east-1:000000000000:alarms\"}}",
                &none,
                Decision::Allowed,
            ),
            (
                "StringNotEqualsIfExists on a modelled key the request carries no value for",
                "{\"StringNotEqualsIfExists\":{\"aws:SourceArn\":\"anything\"}}",
                &none,
                Decision::Allowed,
            ),
            (
                "BoolIfExists on a modelled key the request carries no value for",
                "{\"BoolIfExists\":{\"aws:SecureTransport\":\"false\"}}",
                &none,
                Decision::Allowed,
            ),
            (
                "ForAnyValue:StringEqualsIfExists, where the suffix answers the empty set \
                 the qualifier would read as false",
                "{\"ForAnyValue:StringEqualsIfExists\":{\"aws:SourceArn\":[\"anything\"]}}",
                &none,
                Decision::Allowed,
            ),
            (
                "the suffix on a key the request does not model, which stays undecided",
                "{\"ArnEqualsIfExists\":{\"aws:SourceVpc\":\"vpc-1\"}}",
                &none,
                Decision::NotStated,
            ),
            (
                "NullIfExists, which IAM defines for no operator",
                "{\"NullIfExists\":{\"aws:SourceArn\":\"true\"}}",
                &none,
                Decision::NotStated,
            ),
        ];
        for (case, condition, context, expected) in rows {
            let document = identity(&format!(
                "{{\"Effect\":\"Allow\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\",\
                 \"Condition\":{condition}}}"
            ));
            let request = Request::new("rum:PutRumEvents", RESOURCE, &held)
                .with_context(context)
                .with_published_keys(&published);
            let reached = identity_decision(&[document], &request);
            assert_eq!(
                reached, expected,
                "case {case:?}: expected {expected:?}, got {reached:?}"
            );
        }

        // The suffix decides a DENY the same way: a clause that holds because the key is
        // absent makes its statement apply, so the deny refuses.
        let denial = identity(
            "{\"Effect\":\"Deny\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\",\
             \"Condition\":{\"ArnEqualsIfExists\":{\"aws:SourceArn\":\"arn:aws:sns:*\"}}}",
        );
        let request = Request::new("rum:PutRumEvents", RESOURCE, &held)
            .with_context(&none)
            .with_published_keys(&published);
        let reached = identity_decision(&[denial], &request);
        assert_eq!(
            reached,
            Decision::Denied,
            "case 'a deny whose IfExists clause holds on an absent key': expected the deny to \
             apply, got {reached:?}"
        );
    }

    #[test]
    fn a_set_qualifier_decides_a_key_the_request_carries_several_values_for() {
        let statement = |condition: &str| {
            identity(&format!(
                "{{\"Effect\":\"Allow\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\",\
                 \"Condition\":{condition}}}"
            ))
        };
        let held = principals();
        // Two values for one key, as a caller passes a multi-valued condition key.
        let two = [
            ("aws:TagKeys".to_string(), "Project".to_string()),
            ("aws:TagKeys".to_string(), "Owner".to_string()),
        ];
        let one = [("aws:TagKeys".to_string(), "Project".to_string())];
        let none: [(String, String); 0] = [];

        // The rows differ along what the rule generalises over: the qualifier, how many
        // values the request carries (two, one, none), whether the clause's values cover
        // them, and whether the request MODELS the key at all.
        type QualifierRow<'a> = (&'a str, &'a str, &'a [(String, String)], Decision);
        let rows: [QualifierRow<'_>; 8] = [
            (
                "ForAllValues over both values, both covered",
                "{\"ForAllValues:StringEquals\":{\"aws:TagKeys\":[\"Project\",\"Owner\"]}}",
                &two,
                Decision::Allowed,
            ),
            (
                "ForAllValues over both values, only one covered",
                "{\"ForAllValues:StringEquals\":{\"aws:TagKeys\":[\"Project\"]}}",
                &two,
                Decision::NotStated,
            ),
            (
                "ForAllValues over one value, covered",
                "{\"ForAllValues:StringEquals\":{\"aws:TagKeys\":[\"Project\",\"Owner\"]}}",
                &one,
                Decision::Allowed,
            ),
            (
                "ForAllValues over a modelled key the request carries no value for, \
                 which IAM holds true",
                "{\"ForAllValues:StringEquals\":{\"aws:TagKeys\":[\"Project\"]}}",
                &none,
                Decision::Allowed,
            ),
            (
                "ForAnyValue over both values, one covered",
                "{\"ForAnyValue:StringEquals\":{\"aws:TagKeys\":[\"Owner\"]}}",
                &two,
                Decision::Allowed,
            ),
            (
                "ForAnyValue over both values, none covered",
                "{\"ForAnyValue:StringEquals\":{\"aws:TagKeys\":[\"Team\"]}}",
                &two,
                Decision::NotStated,
            ),
            (
                "ForAnyValue over a modelled key the request carries no value for, \
                 which IAM holds false",
                "{\"ForAnyValue:StringEquals\":{\"aws:TagKeys\":[\"Project\"]}}",
                &none,
                Decision::NotStated,
            ),
            (
                "an unqualified operator on a key carrying several values",
                "{\"StringEquals\":{\"aws:TagKeys\":\"Project\"}}",
                &two,
                Decision::NotStated,
            ),
        ];

        // The key every row above declares the request MODELS, so an empty set is the
        // caller passing no values rather than this evaluator not knowing the key.
        let published = ["aws:TagKeys"];
        for (case, condition, context, expected) in rows {
            let request = Request::new("rum:PutRumEvents", RESOURCE, &held)
                .with_context(context)
                .with_published_keys(&published);
            let reached = identity_decision(&[statement(condition)], &request);
            assert_eq!(
                reached, expected,
                "case {case:?}: expected {expected:?}, got {reached:?}"
            );
        }

        // A key the request does NOT model is undecided whatever the qualifier says, so
        // neither spelling turns an `Allow` into a grant: absence there means this
        // evaluator was told nothing about the key, not that the caller passed no values.
        for (case, condition) in [
            (
                "ForAllValues over a key the request does not model",
                "{\"ForAllValues:StringEquals\":{\"aws:SourceVpc\":[\"vpc-1\"]}}",
            ),
            (
                "ForAnyValue over a key the request does not model",
                "{\"ForAnyValue:StringEquals\":{\"aws:SourceVpc\":[\"vpc-1\"]}}",
            ),
        ] {
            let request = Request::new("rum:PutRumEvents", RESOURCE, &held)
                .with_context(&two)
                .with_published_keys(&published);
            let reached = identity_decision(&[statement(condition)], &request);
            assert_eq!(
                reached,
                Decision::NotStated,
                "case {case:?}: expected the clause to grant nothing, got {reached:?}"
            );
        }

        // A wildcard pattern under a qualifier is the same matcher the unqualified
        // operator uses.
        let request = Request::new("rum:PutRumEvents", RESOURCE, &held)
            .with_context(&two)
            .with_published_keys(&published);
        let reached = identity_decision(
            &[statement(
                "{\"ForAllValues:StringLike\":{\"aws:TagKeys\":[\"Pro*\",\"Own*\"]}}",
            )],
            &request,
        );
        assert_eq!(
            reached,
            Decision::Allowed,
            "case 'ForAllValues:StringLike': expected the wildcards to cover both values, got \
             {reached:?}"
        );
    }

    /// A published key FAMILY covers the keys under it, and nothing else — including a key
    /// whose multi-byte character straddles the family's own length, which the comparison
    /// must answer rather than panic on.
    #[test]
    fn a_published_key_family_covers_the_keys_under_it() {
        let statement = |condition: &str| {
            identity(&format!(
                "{{\"Effect\":\"Allow\",\"Action\":\"sqs:SendMessage\",\"Resource\":\"*\",\
                 \"Condition\":{condition}}}"
            ))
        };
        let family = ["aws:PrincipalTag/"];
        let held = principals();
        let tagged = [("aws:PrincipalTag/team".to_string(), "payments".to_string())];
        // (case, the condition, what the document then states)
        let rows: &[(&str, &str, Decision)] = &[
            (
                "a key of the family the request carries",
                "{\"Null\":{\"aws:PrincipalTag/team\":\"false\"}}",
                Decision::Allowed,
            ),
            (
                "a key of the family the request does not carry",
                "{\"Null\":{\"aws:PrincipalTag/project\":\"true\"}}",
                Decision::Allowed,
            ),
            (
                "the same key asserted present, which it is not",
                "{\"Null\":{\"aws:PrincipalTag/project\":\"false\"}}",
                Decision::NotStated,
            ),
            (
                "a key outside the family",
                "{\"Null\":{\"aws:ResourceTag/team\":\"true\"}}",
                Decision::NotStated,
            ),
            (
                // Sixteen ASCII bytes then a two-byte character, so the key is longer than
                // the family in bytes while the family's length falls inside a character.
                "a key whose multi-byte character straddles the family's length",
                "{\"Null\":{\"aws:PrincipalTagé\":\"true\"}}",
                Decision::NotStated,
            ),
        ];
        for (case, condition, expected) in rows {
            let request = Request::new("sqs:SendMessage", RESOURCE, &held)
                .with_context(&tagged)
                .with_published_keys(&family);
            let reached = identity_decision(&[statement(condition)], &request);
            assert_eq!(
                reached, *expected,
                "case {case:?}: expected {expected:?}, got {reached:?}"
            );
        }
    }

    /// A family whose members are named after it by a COLON — the form a service's
    /// per-entry condition keys take — covers its members the same way a slash-separated
    /// one does, and covers nothing outside itself.
    #[test]
    fn a_colon_separated_published_family_covers_the_keys_under_it() {
        let statement = |condition: &str| {
            identity(&format!(
                "{{\"Effect\":\"Allow\",\"Action\":\"kms:Decrypt\",\"Resource\":\"*\",\
                 \"Condition\":{condition}}}"
            ))
        };
        let family = ["kms:EncryptionContext:"];
        let held = principals();
        let carried = [(
            "kms:EncryptionContext:tenant".to_string(),
            "payments".to_string(),
        )];
        // (case, the condition, what the document then states)
        let rows: &[(&str, &str, Decision)] = &[
            (
                "a member of the family the request carries",
                "{\"StringEquals\":{\"kms:EncryptionContext:tenant\":\"payments\"}}",
                Decision::Allowed,
            ),
            (
                // Decided as absent, which is what declaring the family buys: a negated
                // operator holds on a member the request carries no value for.
                "a member of the family the request does not carry",
                "{\"StringNotEquals\":{\"kms:EncryptionContext:zone\":\"a\"}}",
                Decision::Allowed,
            ),
            (
                "the same member under a positive operator",
                "{\"StringEquals\":{\"kms:EncryptionContext:zone\":\"a\"}}",
                Decision::NotStated,
            ),
            (
                "a member asserted absent through the existence operator",
                "{\"Null\":{\"kms:EncryptionContext:zone\":\"true\"}}",
                Decision::Allowed,
            ),
            (
                "a key outside the family",
                "{\"Null\":{\"kms:ViaService\":\"true\"}}",
                Decision::NotStated,
            ),
        ];
        for (case, condition, expected) in rows {
            let request = Request::new("kms:Decrypt", RESOURCE, &held)
                .with_context(&carried)
                .with_published_keys(&family);
            let reached = identity_decision(&[statement(condition)], &request);
            assert_eq!(
                reached, *expected,
                "case {case:?}: expected {expected:?}, got {reached:?}"
            );
        }
    }

    /// The ARN of the IAM user the policy-variable rows below are decided about: the
    /// principal that signed, which is also the resource it addresses.
    const CALLER: &str = "arn:aws:iam::000000000000:user/writer";

    /// The condition context a request signed by that user carries: the keys that
    /// describe the signing principal, which is where a variable's value comes from. One
    /// tag's value is a `*`, so a row can state what a value containing a wildcard
    /// character means, and one key carries two values.
    fn caller_context() -> Vec<(String, String)> {
        vec![
            ("aws:username".to_string(), "writer".to_string()),
            ("aws:PrincipalTag/team".to_string(), "payments".to_string()),
            ("aws:PrincipalTag/glob".to_string(), "*".to_string()),
            ("aws:TagKeys".to_string(), "Project".to_string()),
            ("aws:TagKeys".to_string(), "Owner".to_string()),
        ]
    }

    /// A policy variable is a placeholder for the value the REQUEST carries for the
    /// condition key it names, substituted before the element is matched — and the
    /// substituted text is text, never a pattern.
    #[test]
    fn a_policy_variable_is_substituted_from_the_request_context() {
        // The rows differ along what the rule generalises over: which key the variable
        // names, the case it is spelled in, where it sits in the element, whether a
        // wildcard belongs to the document, to an escape or to a value, and whether the
        // request carries one value for the key, none, or several.
        let rows: &[(&str, &str, Decision)] = &[
            (
                "the signing user's name, which is what the ARN carries",
                "arn:aws:iam::000000000000:user/${aws:username}",
                Decision::Allowed,
            ),
            (
                "the same key spelled in another case",
                "arn:aws:iam::000000000000:user/${AWS:UserName}",
                Decision::Allowed,
            ),
            (
                "a wildcard the document wrote after the variable",
                "arn:aws:iam::000000000000:user/${aws:username}*",
                Decision::Allowed,
            ),
            (
                "a variable spanning the whole name part of another entity",
                "arn:aws:iam::000000000000:role/${aws:username}",
                Decision::NotStated,
            ),
            (
                "a principal tag whose value is not the name",
                "arn:aws:iam::000000000000:user/${aws:PrincipalTag/team}",
                Decision::NotStated,
            ),
            (
                "a value that is a star, which is that character and not every user",
                "arn:aws:iam::000000000000:user/${aws:PrincipalTag/glob}",
                Decision::NotStated,
            ),
            (
                "a key the request carries no value for",
                "arn:aws:iam::000000000000:user/${aws:PrincipalTag/absent}",
                Decision::NotStated,
            ),
            (
                "a key the request carries several values for",
                "arn:aws:iam::000000000000:user/${aws:TagKeys}",
                Decision::NotStated,
            ),
            (
                "the dollar escape, after which nothing is a variable",
                "arn:aws:iam::000000000000:user/${$}{aws:username}",
                Decision::NotStated,
            ),
            (
                "the star escape, which is the character and not a wildcard",
                "arn:aws:iam::000000000000:user/writer${*}",
                Decision::NotStated,
            ),
            (
                "the question-mark escape, which is the character",
                "arn:aws:iam::000000000000:user/writer${?}",
                Decision::NotStated,
            ),
            (
                "an opener the document never closes, which opens no variable",
                "arn:aws:iam::000000000000:user/${aws:username",
                Decision::NotStated,
            ),
            (
                "no variable at all",
                "arn:aws:iam::000000000000:user/*",
                Decision::Allowed,
            ),
        ];
        let held = principals();
        let context = caller_context();
        for (case, resource, expected) in rows {
            let document = identity(&format!(
                "{{\"Effect\":\"Allow\",\"Action\":\"iam:ListAccessKeys\",\
                 \"Resource\":\"{resource}\"}}"
            ));
            let request = Request::new("iam:ListAccessKeys", CALLER, &held).with_context(&context);
            let reached = identity_decision(&[document], &request);
            assert_eq!(
                reached, *expected,
                "case {case:?}: expected {expected:?}, got {reached:?}"
            );
        }
    }

    /// A clause's own values carry variables too, and an element naming a variable the
    /// request cannot supply is read fail-closed in both directions: it grants nothing,
    /// and a `Deny` carrying it still refuses.
    #[test]
    fn a_variable_the_request_cannot_supply_grants_nothing_and_still_refuses() {
        let held = principals();
        let context = caller_context();
        let decide = |statements: &str| {
            let request = Request::new("iam:ListAccessKeys", CALLER, &held).with_context(&context);
            identity_decision(
                &[format!("{{\"Version\":\"2012-10-17\",\"Statement\":[{statements}]}}")],
                &request,
            )
        };

        // (case, the statements of one document, what it then states)
        let rows: &[(&str, &str, Decision)] = &[
            (
                "a condition value that expands to the key's own value",
                "{\"Effect\":\"Allow\",\"Action\":\"iam:ListAccessKeys\",\"Resource\":\"*\",\
                 \"Condition\":{\"StringEquals\":{\"aws:PrincipalTag/team\":\
                 \"${aws:PrincipalTag/team}\"}}}",
                Decision::Allowed,
            ),
            (
                "a condition value that expands to another key's value",
                "{\"Effect\":\"Allow\",\"Action\":\"iam:ListAccessKeys\",\"Resource\":\"*\",\
                 \"Condition\":{\"StringEquals\":{\"aws:PrincipalTag/team\":\
                 \"${aws:username}\"}}}",
                Decision::NotStated,
            ),
            (
                "a condition value naming a key the request cannot supply",
                "{\"Effect\":\"Allow\",\"Action\":\"iam:ListAccessKeys\",\"Resource\":\"*\",\
                 \"Condition\":{\"StringEquals\":{\"aws:PrincipalTag/team\":\
                 \"${aws:PrincipalTag/absent}\"}}}",
                Decision::NotStated,
            ),
            (
                "a Deny whose resource names a variable the request cannot supply, \
                 beside a grant",
                "{\"Effect\":\"Allow\",\"Action\":\"iam:ListAccessKeys\",\"Resource\":\"*\"},\
                 {\"Effect\":\"Deny\",\"Action\":\"iam:ListAccessKeys\",\
                 \"Resource\":\"arn:aws:iam::000000000000:user/${aws:PrincipalTag/absent}\"}",
                Decision::Denied,
            ),
            (
                "a resource element one of whose values does match",
                "{\"Effect\":\"Allow\",\"Action\":\"iam:ListAccessKeys\",\"Resource\":\
                 [\"arn:aws:iam::000000000000:user/${aws:PrincipalTag/absent}\",\
                 \"arn:aws:iam::000000000000:user/${aws:username}\"]}",
                Decision::Allowed,
            ),
        ];
        for (case, statements, expected) in rows {
            let reached = decide(statements);
            assert_eq!(
                reached, *expected,
                "case {case:?}: expected {expected:?}, got {reached:?}"
            );
        }
    }

    /// Substituting a value longer than the bound every policy document is held to leaves
    /// an element nothing matches, which is what keeps the matcher's work bounded.
    #[test]
    fn an_element_whose_variable_expands_past_the_document_bound_matches_nothing() {
        let held = principals();
        // One value longer on its own than the whole bound, so the expanded element could
        // not have been a document this evaluator reads at all.
        let context = [(
            "aws:PrincipalTag/huge".to_string(),
            "a".repeat(MAX_POLICY_BYTES + 1),
        )];
        let document = identity(
            "{\"Effect\":\"Allow\",\"Action\":\"iam:ListAccessKeys\",\
             \"Resource\":\"arn:aws:iam::000000000000:user/${aws:PrincipalTag/huge}\"}",
        );
        let request = Request::new("iam:ListAccessKeys", CALLER, &held).with_context(&context);
        let reached = identity_decision(&[document], &request);
        assert_eq!(
            reached,
            Decision::NotStated,
            "case 'a value past the document bound': expected the element to match nothing, \
             got {reached:?}"
        );
    }

    #[test]
    fn the_wildcard_matcher_follows_iam_s_two_wildcards() {
        let rows: [(&str, &str, &str, bool, bool); 11] = [
            ("an exact match", "s3:GetObject", "s3:GetObject", true, true),
            ("a trailing star", "s3:*", "s3:GetObject", true, true),
            ("a star in the middle", "s3:Get*ect", "s3:GetObject", true, true),
            ("two stars", "*Get*", "s3:GetObject", true, true),
            ("a question mark", "s3:GetObjec?", "s3:GetObject", true, true),
            (
                "a question mark that must match one character",
                "s3:GetObject?",
                "s3:GetObject",
                true,
                false,
            ),
            ("a star matching nothing", "s3:GetObject*", "s3:GetObject", true, true),
            ("a non-match", "s3:Put*", "s3:GetObject", true, false),
            ("case folding off", "S3:GETOBJECT", "s3:GetObject", true, false),
            ("case folding on", "S3:GETOBJECT", "s3:GetObject", false, true),
            ("every value", "*", "", true, true),
        ];
        for (case, pattern, value, case_sensitive, expected) in rows {
            let matched = wildcard_match(pattern, value, case_sensitive);
            assert_eq!(
                matched, expected,
                "case {case:?}: {pattern:?} against {value:?} (case_sensitive={case_sensitive}) \
                 expected {expected}, got {matched}"
            );
        }
    }

    #[test]
    fn an_accounts_spellings_come_from_the_principals_own_arn() {
        let rows: [(&str, &str, Vec<String>); 6] = [
            (
                "a user of the commercial partition",
                "arn:aws:iam::000000000000:user/writer",
                vec![
                    "arn:aws:iam::000000000000:root".to_string(),
                    "000000000000".to_string(),
                ],
            ),
            (
                "a role under a path",
                "arn:aws:iam::123456789012:role/team/writer",
                vec![
                    "arn:aws:iam::123456789012:root".to_string(),
                    "123456789012".to_string(),
                ],
            ),
            (
                "an assumed-role session of another service's namespace",
                "arn:aws:sts::123456789012:assumed-role/writer/session",
                vec![
                    "arn:aws:iam::123456789012:root".to_string(),
                    "123456789012".to_string(),
                ],
            ),
            (
                "another partition keeps its own",
                "arn:aws-cn:iam::123456789012:user/writer",
                vec![
                    "arn:aws-cn:iam::123456789012:root".to_string(),
                    "123456789012".to_string(),
                ],
            ),
            (
                "an account-less ARN names no account",
                "arn:aws:s3:::reports/june.csv",
                Vec::new(),
            ),
            ("not an ARN at all", "writer", Vec::new()),
        ];
        for (case, arn, expected) in rows {
            let spellings = account_principals(arn);
            assert_eq!(
                spellings, expected,
                "case {case:?}: expected {expected:?}, got {spellings:?}"
            );
        }
    }

    /// RULE: documents could allow an action only if one of them holds an `Allow` whose
    /// action element covers it — whichever resource that statement is scoped to, and
    /// whatever condition it carries — because an `Allow` is the only thing that grants.
    #[test]
    fn documents_could_allow_only_an_action_an_allow_statement_names() {
        // (case, the statements of the one document, whether `rum:PutRumEvents` could be
        // allowed by them)
        let rows: [(&str, &str, bool); 10] = [
            ("no statement at all", "", false),
            (
                "an allow of the action over every resource",
                "{\"Effect\":\"Allow\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\"}",
                true,
            ),
            (
                "an allow scoped to one resource ARN",
                "{\"Effect\":\"Allow\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"arn:aws:rum:us-east-1:000000000000:appmonitor/shop\"}",
                true,
            ),
            (
                "an allow of the action in another case",
                "{\"Effect\":\"Allow\",\"Action\":\"RUM:putrumevents\",\"Resource\":\"*\"}",
                true,
            ),
            (
                "an allow under a wildcard over the service",
                "{\"Effect\":\"Allow\",\"Action\":\"rum:*\",\"Resource\":\"*\"}",
                true,
            ),
            (
                "an allow of another service's action",
                "{\"Effect\":\"Allow\",\"Action\":\"secretsmanager:*\",\"Resource\":\"*\"}",
                false,
            ),
            (
                "a DENY of the action and nothing else",
                "{\"Effect\":\"Deny\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\"}",
                false,
            ),
            (
                "an allow whose condition this cannot decide",
                "{\"Effect\":\"Allow\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\",\
                 \"Condition\":{\"StringEquals\":{\"rum:alias\":\"shop\"}}}",
                true,
            ),
            (
                "an allow of everything but this action",
                "{\"Effect\":\"Allow\",\"NotAction\":\"rum:PutRumEvents\",\"Resource\":\"*\"}",
                false,
            ),
            (
                "an allow of the action beside one of another",
                "{\"Effect\":\"Allow\",\"Action\":\"s3:GetObject\",\"Resource\":\"*\"},\
                 {\"Effect\":\"Allow\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\"}",
                true,
            ),
        ];
        for (case, text, expected) in rows {
            let documents = prepare_documents(&[identity(text)]);
            let could = identity_may_allow_action(&documents, "rum:PutRumEvents");
            assert_eq!(
                could, expected,
                "case {case:?}: expected identity_may_allow_action {expected}, got {could}"
            );
        }
    }
}
