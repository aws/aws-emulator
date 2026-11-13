// SPDX-License-Identifier: CC0-1.0

//! WHO a data-plane request speaks for, and whether the policies in force let it act on
//! the table it addresses.
//!
//! A table's resource-based policy is evaluated on every call that reads or writes an item
//! of that table, at the instant of the call — nothing is cached, so a policy just put
//! refuses immediately and one just deleted stops refusing immediately.
//!
//! The pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for, read from the SAME
//!    place `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the
//!    one the caller reads back from STS for the credentials it signed with. Resolving it
//!    materialises the session expiries the clock has already made due, a write owed at
//!    that instant whether or not anybody called.
//! 2. **what the TABLE's own document says** — the revision of [`super::AttachedPolicy`]
//!    in force at the instant of the call ([`super::ResourcePolicySlot::visible`]): a
//!    `Deny` about the request refuses it whatever any grant says, and an `Allow` that
//!    names the caller itself grants it.
//! 3. **what the CALLER is allowed to do at all** — the identity-based documents the IAM
//!    store holds for that principal. Both halves are judged by
//!    [`::service_common::iam_authorization`], the tree's one evaluation of policy
//!    documents against `(action, resource, principal, context)`, so this service and
//!    every other one handed the same documents decide alike — a resource policy's
//!    statement `Condition` included.
//! 4. **what the CREDENTIAL itself was scoped to** — the session policies the mint of the
//!    credential passed, which STS holds on the session the access key id names
//!    ([`sts_store::session_scoping_documents`]) and which are read by the one reading of a
//!    resolved scope ([`sts_store::ScopingPolicies::decision`]). They SCOPE the permissions
//!    of the
//!    identity down and grant nothing of their own: "the resulting session's permissions
//!    are the intersection of the role's identity-based policies and the session policies"
//!    (`AssumeRoleRequest$Policy`). A long-term access key is scoped by none.
//!
//! ## The rule
//!
//! In IAM's own order: an explicit `Deny` about the request — in the table's policy, in an
//! identity-based document or in a session policy — refuses, whatever any grant says.
//! Otherwise the request needs a grant: an `Allow` about it in the caller's identity-based
//! policies, or one in the table's policy that names the CALLER. With neither the request is
//! refused, which is IAM's implicit deny, and the operation has no effect. A grant still has
//! to pass the scope the credential was minted with: a request the session policies do not
//! allow is refused even where the documents in force on the identity allow it, and one only
//! they allow is refused as if they had allowed nothing — each refusal naming the document
//! that withheld the grant, because they need different fixes.
//!
//! A statement that names the ACCOUNT rather than the caller (the account's root ARN, or
//! the bare account id that stands for it) delegates to the account rather than granting:
//! "you must also attach an identity-based policy" to the principal. So the account
//! spellings decide a `Deny` — which covers every principal of the account — and never
//! make a grant on their own.
//!
//! ## The two callers that are not IAM principals
//!
//! * **The account's own published credential**, which is the position AWS account root
//!   credentials are in: the IAM store holds no policies for it and the table's owner has
//!   full control of its tables, so such a request is allowed — while an explicit `Deny`
//!   in the table's policy still refuses it.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed
//!   policy is owned by AWS, not by the account, so the account's store holds no document
//!   for it; refusing on evidence this emulator does not have would deny access AWS
//!   permits, so such a principal is allowed where the readable documents neither grant
//!   nor deny. A credential scoped by a MANAGED session policy whose document is likewise
//!   unheld is in that same position: the scope it was minted with cannot withhold a grant
//!   this emulator cannot read, while a `Deny` a document it CAN read still refuses.
//!
//! An expired session credential speaks for nobody: no statement names it, so every action
//! it asks for is refused.
//!
//! ## What the request itself contributes to the decision
//!
//! A statement may be CONDITIONED, and a condition is decided against the keys the request
//! carries values for. Two of them belong to a data-plane call of this service:
//!
//! * the partition this call addresses — [`AddressedKeys`], the value(s) of the table's
//!   `HASH` key attribute the `Key`, the `Item`, the key condition or the statement names,
//!   one per partition the call touches. A call that addresses no partition of its own (a
//!   table or index walk) carries no value for it and models none, so a clause over it
//!   cannot be decided and the statement carrying that clause neither grants nor is
//!   weakened.
//! * the session tags the calling principal carries, which is where a policy written with
//!   `${aws:PrincipalTag/<key>}` takes the text it stands for. They are known for a
//!   session this emulator's STS registry minted and for nobody else, so only such a
//!   caller declares that an absent tag key is a tag it does not carry.
//! * the federated identity the calling credential was minted by exchanging, which a
//!   session of a role assumed with a web identity token carries: the
//!   `<provider>:<claim>` keys that token publishes, among them the subject it names. They
//!   are what a policy written with `${<provider>:<claim>}` takes the text it stands for,
//!   so a statement conditioned on the partitions a call addresses can pin them to the
//!   caller's OWN federated subject.
//!
//! Both are passed through [`::service_common::iam_authorization::Request`] — for the
//! table's own document as much as for the identity's — so the clause is decided by the
//! tree's one evaluation of policy documents rather than here. A fine-grained statement
//! over the partitions a call addresses therefore decides that call: it applies to the
//! partitions its condition selects and to no others.

use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

use super::*;

/// The condition key whose value is the partition a data-plane call addresses: the first
/// key attribute of the table, which is what a fine-grained identity policy scopes a
/// caller to one partition with.
const LEADING_KEYS_CONDITION_KEY: &str = "dynamodb:LeadingKeys";

/// The condition-key FAMILY the calling principal's session tags are named in, each member
/// after the tag key it carries: `aws:PrincipalTag/tenant` holds the value of the session
/// tag `tenant`.
const PRINCIPAL_TAG_CONDITION_KEY_FAMILY: &str = "aws:PrincipalTag/";

/// The principal a request speaks for, and everything the decision is made from.
///
/// Resolved once per request, BEFORE this service's own store is locked, so the STS and
/// IAM stores are never reached while this service's state guard is held.
#[derive(Debug, Clone, Default)]
pub struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    pub arn: String,
    /// Every spelling of the principal a policy may name IT by: the ARN STS reports, and
    /// the ARN of the IAM role behind an assumed-role session. These decide a GRANT.
    pub principals: Vec<String>,
    /// The spellings that name the ACCOUNT the principal belongs to rather than the
    /// principal: they decide a deny and never grant (see the module docs).
    pub account_principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    pub documents: Vec<String>,
    /// The session policy documents the mint of this credential scoped it with, as they
    /// read at the instant of this request, and whether a managed one among them names a
    /// document the IAM store does not hold (see the module docs). Empty for a credential
    /// no mint scoped.
    pub scope: sts_store::ScopingPolicies,
    /// Set when this request presents the account's own published credential, which holds
    /// no IAM policies (see the module docs).
    pub account_credential: bool,
    /// Set when the principal carries a policy whose document the IAM store does not hold
    /// — an AWS-managed policy (see the module docs).
    pub unreadable_attachment: bool,
    /// The session tags the principal carries, as STS attached them when the session was
    /// minted: the values `aws:PrincipalTag/<key>` holds for a request this credential
    /// signs.
    pub tags: Vec<(String, String)>,
    /// Set when the principal's session tags are KNOWN — a session this emulator's STS
    /// registry holds — which is what lets a clause over a tag key the principal does NOT
    /// carry be decided rather than left undecided. A long-term credential's tags live in
    /// IAM and are not read here, so such a caller declares nothing about them.
    pub tags_known: bool,
    /// The condition keys the web identity this credential was minted by exchanging
    /// publishes, as STS recorded them on the session: the values a
    /// `<provider>:<claim>` clause, and a policy variable naming one, hold for a request
    /// this credential signs. Empty for a credential no such exchange minted.
    pub federated_claims: Vec<(String, String)>,
}

impl Caller {
    /// Every spelling a DENY is decided against: the principal's own and its account's,
    /// because a statement denying the account denies each of its principals.
    fn denied_spellings(&self) -> Vec<String> {
        let mut named = self.principals.clone();
        named.extend(self.account_principals.iter().cloned());
        named
    }
}

/// Who this request speaks for, resolved from the access key id it signed with.
///
/// Call it before locking this service's store (see [`Caller`]).
pub fn request_caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let iam = sts_store::iam_state(engine);
    // A long-term access key speaks for the IAM user that holds it, which is the identity
    // STS itself reports for it; a key no store holds is the account's own published
    // credential.
    let long_term = match &iam {
        Some(iam) => sts_store::long_term_identity(Some(&iam.lock()), &access_key_id),
        None => sts_store::ambient_identity(),
    };
    let resolved = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_request_caller(
            &mut sts.lock(),
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
            long_term.clone(),
        )
        .map(|caller| {
            (
                caller.identity.arn.clone(),
                caller.identity.account.clone(),
                caller.role_arn.clone(),
                // WHOSE identity-based documents are in force for this credential, which is
                // not always the principal it speaks for: STS's one answer for it
                // (`sts_store::Caller::identity_policies_arn`).
                caller.identity_policies_arn().to_string(),
                // The tags are this registry's record of the session, so they are known
                // exactly for a credential it minted.
                caller.kind.is_some(),
                caller.tags.clone(),
                caller.session_policies.clone(),
                caller.federated_claims.clone(),
            )
        }),
        // No STS store on this engine: the credential names no session, so it speaks for
        // the long-term identity it was resolved to, and is judged on that identity's
        // documents.
        None => Ok((
            long_term.arn.clone(),
            long_term.account.clone(),
            None,
            long_term.arn.clone(),
            false,
            Vec::new(),
            sts_store::SessionPolicies::default(),
            Vec::new(),
        )),
    };
    // An expired session credential speaks for nobody: it carries no documents and no
    // principal spelling. The refusal still names what was presented, so a caller reading
    // it can tell an expired credential from an unauthorized one.
    let Ok((
        arn,
        account,
        role_arn,
        policies_arn,
        tags_known,
        tags,
        session_policies,
        federated_claims,
    )) = resolved
    else {
        return Caller {
            arn: format!("an expired session credential ({access_key_id})"),
            ..Caller::default()
        };
    };
    let mut caller = Caller {
        account_credential: arn == sts_store::ambient_identity().arn,
        principals: vec![arn.clone()],
        account_principals: iam::account_principals(&arn),
        arn,
        tags,
        tags_known,
        federated_claims,
        ..Caller::default()
    };
    // An assumed-role session is also named by the ARN of the role it is a session of,
    // which is the spelling a policy uses to name the role itself, and the identity whose
    // policies are in force for it.
    if let Some(role_arn) = &role_arn {
        if !caller.principals.contains(role_arn) {
            caller.principals.push(role_arn.clone());
        }
    }
    if caller.account_principals.is_empty() {
        caller.account_principals = vec![account_root_arn(&account), account];
    }
    // The IAM store of the same engine, read ONCE for both halves of what the caller may do:
    // the documents in force on its identity, and the documents its credential was scoped
    // with. An engine carrying no IAM store holds neither. The documents are the ones the
    // IDENTITY behind the credential carries (`policies_arn`), which for a federated-user
    // session is the IAM user the mint stood on rather than the federated principal, who
    // carries none of its own.
    let held = iam.as_ref().map(|iam| iam.lock());
    if let Some(iam) = held.as_deref() {
        if let Some(policies) = iam.principal_policies(&policies_arn) {
            caller.documents = policies.documents;
            caller.unreadable_attachment = !policies.uncarried.is_empty();
        }
    }
    // What the credential itself was scoped to, read at the instant of the request so a
    // managed scoping policy edited since the mint scopes by what it says NOW; the inline
    // document the mint passed scopes it whether or not a store is there to read.
    let scoping = sts_store::session_scoping_documents(held.as_deref(), &session_policies);
    caller.scope = scoping;
    caller
}

/// The ARN of an account's root user, the spelling that stands for the whole account in a
/// policy's `Principal` element.
fn account_root_arn(account: &str) -> String {
    format!("arn:aws:iam::{account}:root")
}

/// The partitions a data-plane call addresses, as the condition key that reports them
/// holds their values: one entry per partition the `Key`, the `Item`, the key condition or
/// the PartiQL statement names, in the order the request named them.
///
/// A call that addresses no partition of its own — a table or index walk — knows no value
/// for the key and MODELS none either, which is what [`Self::whole_table`] stands for: a
/// clause over the key then stays undecided, so a statement carrying it neither grants nor
/// is weakened. A call whose addressed partition cannot be read at all (a key map without
/// the table's own `HASH` attribute, a value of another type than the one the table
/// declared for it) is in that same position, because the value the clause would be
/// decided against is not established — and such a request is refused for what is wrong
/// with it once the decision is past.
#[derive(Debug, Clone, Default)]
pub struct AddressedKeys {
    /// `None` when the call addresses no partition of its own (see the type docs).
    values: Option<Vec<String>>,
}

impl AddressedKeys {
    /// A call that addresses no partition of its own: a table or index walk.
    #[must_use]
    pub fn whole_table() -> Self {
        Self { values: None }
    }

    /// The partitions the key or item maps `maps` address, read against `table`'s own
    /// partition-key attribute.
    #[must_use]
    pub fn of_maps<'a, I: IntoIterator<Item = &'a Item>>(table: &Table, maps: I) -> Self {
        let partition = table.partition_key_name();
        Self::of_values(maps.into_iter().map(|map| map.get(partition)))
    }

    /// The partitions the attribute values `values` address: a `Query`'s partition-key
    /// equality operand, or the value a key map carries for the partition attribute. A
    /// value that is absent, or carries no text a condition key can hold, leaves the
    /// addressed partitions unknown (see the type docs).
    #[must_use]
    pub fn of_values<'a, I: IntoIterator<Item = Option<&'a AttributeValue>>>(values: I) -> Self {
        let mut addressed = Vec::new();
        for value in values {
            match value.and_then(condition_key_text) {
                Some(text) => addressed.push(text),
                None => return Self::whole_table(),
            }
        }
        // A call that named no partition at all addresses the whole table, whatever the
        // member it would have named one in: a request whose addressed partition this
        // could not find is in the position of one that addresses none.
        if addressed.is_empty() {
            return Self::whole_table();
        }
        Self {
            values: Some(addressed),
        }
    }

    /// The partition one PartiQL statement addresses: the item an `INSERT` adds, or the
    /// partition-key equality the `WHERE` clause of a `SELECT`, `UPDATE` or `DELETE` pins.
    /// A `SELECT` that pins none walks the table.
    #[must_use]
    pub fn of_statement(table: &Table, statement: &PartiqlStatement) -> Self {
        let pinned = |predicate: &Predicate| {
            Self::of_values([pinned_equalities(predicate).get(table.partition_key_name())])
        };
        match statement {
            PartiqlStatement::Insert(insert) => Self::of_maps(table, [&insert.item]),
            PartiqlStatement::Select(select) => match &select.filter {
                Some(filter) => pinned(filter),
                None => Self::whole_table(),
            },
            PartiqlStatement::Update(update) => pinned(&update.filter),
            PartiqlStatement::Delete(delete) => pinned(&delete.filter),
        }
    }
}

/// The text a condition key holds for one partition-key value: the string itself for an
/// `S` value, the number in the canonical form this store holds it in for an `N` one (so
/// two spellings of one figure address one partition and decide alike), and the base64 the
/// wire carries for a `B` one — the three types a key attribute may be declared as.
/// Anything else is no key value at all, so it names no text.
///
/// This is the store's ONE rendering of a key attribute's value as the text something outside
/// the item addresses it by, so the partition a policy clause is decided against and the
/// contributor a key activity is attributed to ([`super::contributor_insights`]) are the same
/// string for the same value.
pub(crate) fn condition_key_text(value: &AttributeValue) -> Option<String> {
    match value {
        AttributeValue::S(text) => Some(text.clone()),
        AttributeValue::N(text) => canonical_number_text(text).ok(),
        AttributeValue::B(bytes) => Some(::aws_smithy_types::base64::encode(bytes)),
        _ => None,
    }
}

/// The condition keys this request carries values for, as `(key, value)` pairs: one entry
/// per partition it addresses, one per session tag the calling principal carries, and one
/// per claim of the web identity its credential was minted by exchanging.
fn condition_context(caller: &Caller, addressed: &AddressedKeys) -> Vec<(String, String)> {
    let mut context: Vec<(String, String)> = addressed
        .values
        .iter()
        .flatten()
        .map(|value| (LEADING_KEYS_CONDITION_KEY.to_string(), value.clone()))
        .collect();
    for (key, value) in &caller.tags {
        context.push((
            format!("{PRINCIPAL_TAG_CONDITION_KEY_FAMILY}{key}"),
            value.clone(),
        ));
    }
    context.extend(caller.federated_claims.iter().cloned());
    context
}

/// The condition keys this request MODELS, whether or not it carries a value for each: the
/// keys whose absence from the context means the request carries no value for them rather
/// than that this emulator cannot answer for them.
///
/// A call that addresses partitions knows every one it addresses, and a caller whose
/// session tags are known knows every tag it carries — so the whole tag FAMILY is
/// published, which is what lets a clause over a tag key the principal does not carry be
/// decided. A federated credential answers for each key its own exchange published, so
/// those are published one by one ([`sts_store::federated_claim_keys`]).
fn published_condition_keys<'a>(caller: &'a Caller, addressed: &AddressedKeys) -> Vec<&'a str> {
    let mut published = Vec::new();
    if addressed.values.is_some() {
        published.push(LEADING_KEYS_CONDITION_KEY);
    }
    if caller.tags_known {
        published.push(PRINCIPAL_TAG_CONDITION_KEY_FAMILY);
    }
    published.extend(sts_store::federated_claim_keys(&caller.federated_claims));
    published
}

/// The sentence AWS refuses an unauthorized request with: who was refused, which action on
/// which resource, and the reason the refusal rests on.
///
/// ONE statement of that wording for this crate, so the data-plane's policy decision
/// ([`denial_addressing`]) and every other refusal naming an action the caller may not
/// perform cannot word it differently.
pub fn not_authorized(caller_arn: &str, action: &str, resource: &str, reason: &str) -> String {
    format!(
        "User: {caller_arn} is not authorized to perform: {action} on resource: {resource} \
         {reason}"
    )
}

/// The `AccessDeniedException` message a request that addresses no partition of its own
/// earns when the policies in force do not permit it, or `None` when they do. See
/// [`denial_addressing`], which this is the table-wide case of.
pub fn denial(caller: &Caller, table: &Table, action: &str, now: SystemTime) -> Option<String> {
    denial_addressing(caller, table, action, now, &AddressedKeys::whole_table())
}

/// The `AccessDeniedException` message a data-plane request earns when the policies in
/// force do not permit it, or `None` when they do.
///
/// `action` is the IAM action the operation performs (`dynamodb:GetItem`) and `addressed`
/// the partitions the request acts on. The decision is the module docs' rule, over the
/// table's own policy, the caller's identity-based documents and the session policies its
/// credential was scoped with, with the request's own condition keys supplied so a
/// CONDITIONED statement is decided on the request rather than left undecided; a table with
/// no policy attached — or one whose application is not due yet — states nothing, so such a
/// request is decided by the identity-based half alone, and a credential no mint scoped is
/// capped by nothing.
pub fn denial_addressing(
    caller: &Caller,
    table: &Table,
    action: &str,
    now: SystemTime,
    addressed: &AddressedKeys,
) -> Option<String> {
    let refusal = |reason: &str| Some(not_authorized(&caller.arn, action, &table.arn, reason));
    // "PutResourcePolicy is an asynchronous operation" and "the policy application is
    // eventually consistent" (the `PutResourcePolicy` doc): what decides this request is
    // the revision in force at this instant, which inside the span of the newest write is
    // the one the resource carried before it ([`super::ResourcePolicySlot::visible`]).
    let policy = table.resource_policy.visible(now);
    let context = condition_context(caller, addressed);
    let published = published_condition_keys(caller, addressed);

    // An explicit deny wins over every grant, including the account's own credential. It
    // is decided against every spelling the caller may be named by, its account's
    // included, and over this request's own condition keys, so a statement whose
    // `Condition` this request does not satisfy refuses nothing.
    if let Some(policy) = policy {
        let denied = caller.denied_spellings();
        let denying = iam::Request::new(action, &table.arn, &denied)
            .with_context(&context)
            .with_published_keys(&published);
        if policy.decision(&denying) == Decision::Denied {
            return refusal("with an explicit deny in a resource-based policy");
        }
    }
    let granting = iam::Request::new(action, &table.arn, &caller.principals)
        .with_context(&context)
        .with_published_keys(&published);

    // What the SCOPE the credential was minted with says about the request, read by the
    // tree's one reading of a resolved scope ([`sts_store::ScopingPolicies::decision`]). A
    // session policy caps what the identity's policies grant and grants nothing itself, so
    // a request it explicitly denies is refused whatever any grant says — which is the
    // decision taken here, before the grants are read, because no grant overturns it. A
    // credential scoped by no document this emulator can read is scoped by nothing, which
    // is the position of a session policy that allows the request.
    let scoped = caller.scope.decision(&granting);
    if scoped == Decision::Denied {
        return refusal("with an explicit deny in a session policy");
    }
    // The account's own published credential holds no IAM policies, and the account owns
    // its tables (see the module docs).
    if caller.account_credential {
        return None;
    }

    let decided = iam::identity_decision(&caller.documents, &granting);
    if decided == Decision::Denied {
        return refusal("with an explicit deny in an identity-based policy");
    }
    // Whether anything grants the request at all: an `Allow` in the caller's own
    // documents, or one in the TABLE's policy that names this principal directly, which is
    // a permission of its own ("the resource-based policy and the identity-based policy
    // are evaluated together, and access is granted if either allows it"). A grant this
    // emulator cannot read is not evidence of a refusal (see the module docs), so a
    // principal carrying one is granted where the readable documents neither grant nor
    // deny.
    let granted = decided == Decision::Allowed
        || policy.is_some_and(|policy| policy.decision(&granting) == Decision::Allowed)
        || caller.unreadable_attachment;
    if !granted {
        return refusal(&format!(
            "because no identity-based policy allows the {action} action"
        ));
    }
    // The grant still has to pass the scope the credential was minted with: a request only
    // the identity's policies allow is refused as if the session policies had allowed
    // nothing, and the refusal names THEM as the document that withheld it.
    if scoped != Decision::Allowed {
        return refusal(&format!(
            "because no session policy allows the {action} action"
        ));
    }
    None
}
