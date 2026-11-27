// SPDX-License-Identifier: CC0-1.0

use super::*;

/// The number of logical destinations one account holds: CloudWatch Logs documents a quota
/// of 500 destinations per account per Region ("Destinations: 500 per account per Region",
/// CloudWatch Logs quotas), so a put that would create a 501st destination is refused. The
/// ceiling also bounds this store, whose keys are caller-chosen (`DestinationName` is
/// `@length(min: 1, max: 512)`, so distinct names are effectively unlimited) and whose
/// records stay resident for as long as the process runs: a put that UPDATES a destination
/// already stored is never refused by it, because it adds nothing.
pub const MAX_DESTINATIONS: usize = 500;

/// One logical destination — "a cross-account destination that receives subscription log
/// events", which `PutDestination` creates or updates and `DescribeDestinations` reports —
/// as it is stored. Every member of the model's `Destination` shape is reported from this
/// record, so `DescribeDestinations` answers from state alone.
#[derive(Debug, Clone)]
pub struct DestinationRecord {
    /// `Destination$destinationName` — "The name of the destination." Also the key the
    /// store holds this destination under: it is the only thing `DeleteDestination` and
    /// `PutDestinationPolicy` identify one by.
    pub destination_name: String,
    /// `Destination$arn` — "The ARN of this destination", as [`destination_arn`] builds it.
    pub arn: String,
    /// `Destination$targetArn` — "The Amazon Resource Name (ARN) of the physical target
    /// where the log events are delivered (for example, a Kinesis stream)."
    pub target_arn: String,
    /// `Destination$roleArn` — "A role for impersonation, used when delivering log events
    /// to the target."
    pub role_arn: String,
    /// `Destination$accessPolicy` — "An IAM policy document that governs which Amazon Web
    /// Services accounts can create subscription filters against this destination." `None`
    /// until `PutDestinationPolicy` sets one: "By default, PutDestination does not set any
    /// access policy with the destination [...] the destination owner must call
    /// PutDestinationPolicy after PutDestination."
    pub access_policy: Option<String>,
    /// `Destination$creationTime` — "The creation time of the destination, expressed as the
    /// number of milliseconds after Jan 1, 1970 00:00:00 UTC." Stamped when the destination
    /// is created and kept across later puts, which UPDATE that destination rather than
    /// create one.
    pub creation_time_ms: i64,
    /// The tag set `PutDestination$tags` assigns to the destination — "An optional list of
    /// key-value pairs to associate with the resource" — and the one every tagging
    /// operation reads and writes through
    /// [`ComAmazonawsCloudwatchlogsLogs20140328State::resource_tags`]. A `BTreeMap` so the
    /// stored order is stable.
    pub tags: BTreeMap<String, String>,
}

impl DestinationRecord {
    /// A brand-new destination, with its ARN DERIVED from the request's Region and its own
    /// name rather than taken from the caller, and no access policy: "By default,
    /// PutDestination does not set any access policy with the destination."
    pub fn new(
        region: &str,
        destination_name: &str,
        target_arn: &str,
        role_arn: &str,
        creation_time_ms: i64,
    ) -> Self {
        Self {
            destination_name: destination_name.to_string(),
            arn: destination_arn(region, destination_name),
            target_arn: target_arn.to_string(),
            role_arn: role_arn.to_string(),
            access_policy: None,
            creation_time_ms,
            tags: BTreeMap::new(),
        }
    }

    /// This destination as the model's `Destination` structure, which is what
    /// `PutDestination` and `DescribeDestinations` both report.
    pub fn reported(&self) -> crate::ops::types::Destination {
        crate::ops::types::Destination {
            destination_name: Some(self.destination_name.clone()),
            target_arn: Some(self.target_arn.clone()),
            role_arn: Some(self.role_arn.clone()),
            access_policy: self.access_policy.clone(),
            arn: Some(self.arn.clone()),
            creation_time: Some(self.creation_time_ms),
        }
    }
}

/// A logical destination's ARN, the form `TagResource$resourceArn` documents for one ("The
/// ARN format of a destination is
/// arn:aws:logs:Region:account-id:destination:destination-name"): a `destination` resource
/// of this service, identified by the destination's own name.
pub fn destination_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("logs", region, &format!("{DESTINATION_ARN_PREFIX}{name}"))
}

/// AWS's refusal for a `PutDestination` whose `roleArn` or `targetArn` is not an ARN: the
/// service parses the destination's two ARNs together and reports the failure of either
/// without naming which one failed.
pub const DESTINATION_ARN_PARSE_FAILED: &str = "Error parsing ARN from given role or target";

/// The vendors a logical destination can deliver to, as the ARN service segment naming each.
///
/// A destination "encapsulates a physical resource (such as an Amazon Kinesis stream)" and
/// `PutSubscriptionFilter` states which of them one can be: "A logical destination created
/// with PutDestination that belongs to a different account, for cross-account delivery. We
/// currently support Kinesis Data Streams and Firehose as logical destinations." So a
/// `targetArn` of any other service names a resource this service does not deliver to
/// through a destination, whatever else that resource is.
pub const DESTINATION_TARGET_VENDORS: [&str; 2] = ["kinesis", "firehose"];

/// The refusal CloudWatch Logs answers a `PutDestination` whose `targetArn` names a resource
/// of a vendor a destination cannot deliver to, or `None` for a target it can
/// ([`DESTINATION_TARGET_VENDORS`]) and for a value that is no ARN at all — which names no
/// vendor and is refused for the form it is in instead ([`DESTINATION_ARN_PARSE_FAILED`]).
///
/// The fixed text states that the destination is invalid and why; its one variable part is
/// the vendor, read off the `targetArn` the request carried, so the sentence states the rule
/// rather than one call's target.
pub fn unsupported_target_vendor_message(target_arn: &str) -> Option<String> {
    let (vendor, _resource) = arn_service_and_resource(target_arn)?;
    (!DESTINATION_TARGET_VENDORS.contains(&vendor))
        .then(|| format!("Invalid destination. Vendor {vendor} not supported."))
}

/// What this service states for a put that would create a destination past
/// [`MAX_DESTINATIONS`]: the quota that refused it.
///
/// Stated once here because more than one caller creates a destination — this service's own
/// `PutDestination`, and the resource operations another service runs on one — and the quota
/// is the account's, so both refuse past it with the same sentence.
pub fn destinations_quota_message() -> String {
    format!("an account can have up to {MAX_DESTINATIONS} destinations")
}

/// The shortest name a logical destination is held under — the minimum the model gives
/// `DestinationName` (`@length(min: 1, max: 512)`), so the empty string names none.
pub const MIN_DESTINATION_NAME_CHARS: usize = 1;

/// The longest such name; see [`MIN_DESTINATION_NAME_CHARS`].
pub const MAX_DESTINATION_NAME_CHARS: usize = 512;

/// The characters `DestinationName`'s own `@pattern("^[^:*]*$")` keeps out of one, as a
/// refusal names them.
const DESTINATION_NAME_EXCLUDED_CHARACTERS: [char; 2] = [':', '*'];
/// Whether `value` is a name this service holds a destination under: inside the length its
/// model gives `DestinationName`, and carrying none of the characters its pattern excludes.
///
/// The rule lives here, once, so this service's own wire edge — where the modelled
/// constraints are checked — and any other service that creates a destination on a
/// caller's behalf cannot disagree about which names it holds.
pub fn is_destination_name(value: &str) -> bool {
    let length = value.chars().count();
    (MIN_DESTINATION_NAME_CHARS..=MAX_DESTINATION_NAME_CHARS).contains(&length)
        && !value.contains(DESTINATION_NAME_EXCLUDED_CHARACTERS)
}

/// The requirement a refused destination name failed: the member's WHOLE accepted domain in
/// one sentence, so a name refused for its length and one refused for a character it
/// carries are answered with the same rule.
pub fn destination_name_requirement() -> String {
    let excluded: Vec<String> = DESTINATION_NAME_EXCLUDED_CHARACTERS
        .iter()
        .map(|character| format!("'{character}'"))
        .collect();
    format!(
        "must be {MIN_DESTINATION_NAME_CHARS} to {MAX_DESTINATION_NAME_CHARS} characters and \
         carry none of {}",
        excluded.join(", ")
    )
}

/// The size of the access policy one destination can carry:
/// `PutDestinationPolicy$accessPolicy` is documented "This can be up to 5120 bytes", a ceiling
/// the model's `AccessPolicy` shape does not express (`@length` min 1 only), so the handler
/// holds the document to it. It bounds this store too, since the document is kept verbatim on
/// the destination record.
pub const MAX_ACCESS_POLICY_BYTES: usize = 5_120;

/// The IAM condition key an access policy names an ORGANIZATION by, which is what
/// `PutDestinationPolicy$forceUpdate` ("grant permission to an organization ID instead of
/// granting permission to individual Amazon Web Services accounts") distinguishes a policy by.
pub(super) const ORGANIZATION_PRINCIPAL_CONDITION_KEY: &str = "aws:PrincipalOrgID";

/// Whether an access policy grants permission to an organization rather than to individual
/// accounts, read off the document itself: a policy names the organization through the
/// [`ORGANIZATION_PRINCIPAL_CONDITION_KEY`] condition key.
///
/// The key is matched without regard to case, because IAM condition keys are
/// case-insensitive, and wherever in the document it appears, because a condition can sit
/// under any condition operator of any statement. The walk carries its own stack rather than
/// recursing, so a deeply nested document costs heap instead of call frames. A document that
/// is not JSON at all grants nothing: the handler refuses it before asking.
pub fn access_policy_grants_organization(document: &str) -> bool {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(document) else {
        return false;
    };
    let mut pending = vec![parsed];
    while let Some(value) = pending.pop() {
        match value {
            serde_json::Value::Object(members) => {
                for (key, nested) in members {
                    if key.eq_ignore_ascii_case(ORGANIZATION_PRINCIPAL_CONDITION_KEY) {
                        return true;
                    }
                    pending.push(nested);
                }
            }
            serde_json::Value::Array(items) => pending.extend(items),
            _ => {}
        }
    }
    false
}

/// The refusal CloudWatch Logs answers a resource policy document its IAM policy parser will
/// not take: a document that is not an IAM policy document at all, and one whose statements
/// do not identify the principals they grant to. The text is FIXED — it carries nothing from
/// the request — so every such refusal reads identically.
///
/// It is the parser's own sentence rather than one operation's, so it names the `accessPolicy`
/// the parser is invoked for whichever member of whichever request carried the document
/// (`PutDestinationPolicy$accessPolicy`, `PutResourcePolicy$policyDocument`).
pub const ACCESS_POLICY_PARSE_FAILED: &str =
    "Error occurred while parsing accessPolicy. Please check if the accessPolicy has been \
     constructed correctly using IAM grammar.";

/// Whether `document` is a document CloudWatch Logs' IAM policy parser TAKES, which is what
/// decides between storing it and answering [`ACCESS_POLICY_PARSE_FAILED`].
///
/// A member documented as an IAM policy document carries one in the IAM JSON policy language,
/// and this operation ATTACHES that document to one of this account's resources, so the
/// readings every service of this emulator applies to such a document are applied here, each
/// stated once for the workspace:
///
/// * the document is a policy being attached — a JSON object whose member names the language
///   defines, carrying a statement element and a version string the language defines
///   ([`::service_common::iam_policy::attached_policy_defect`]);
/// * its statements are RESOURCE-policy statements — each naming an effect, the actions it
///   covers and, this being the element that says who the destination is shared with, the
///   principals it grants them to ([`::service_common::iam_policy::resource_policy_defect`]);
/// * every value those principal elements name is a principal the language defines a form for
///   ([`::service_common::iam_policy_principals::unreadable_policy_principal`]), since a value
///   outside them denotes no identity the grant could ever be evaluated against.
///
/// Beyond those, two clauses are this surface's own, and both come from WHAT a destination's
/// access policy is for — "a destination controls what is written to it [...] a cross-account
/// user cannot call PutSubscriptionFilter against this destination [until] the destination
/// owner [...] call[s] PutDestinationPolicy" (`PutDestination`):
///
/// * every statement grants to an ACCOUNT ([`grants_to_an_account`]), because what this policy
///   authorizes is another ACCOUNT's claim on this destination. A grant to a SERVICE principal
///   is a service's permission on a resource, which is not what this document says, so the
///   parser does not take it;
/// * and a statement that ALLOWS the wildcard principal and narrows it with no condition
///   identifies nobody to grant to ([`access_policy_allows_every_principal`]).
///
/// Asked of the DOCUMENT alone, so it can be — and is — asked before the resource the policy
/// would be attached to is resolved: the parse is about the document the request carried, not
/// about this account's state.
pub fn is_access_policy(document: &str) -> bool {
    if ::service_common::iam_policy::attached_policy_defect(document, |_| true).is_some()
        || ::service_common::iam_policy::resource_policy_defect(document).is_some()
        || ::service_common::iam_policy_principals::unreadable_policy_principal(document).is_some()
        || access_policy_allows_every_principal(document)
    {
        return false;
    }
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(document) else {
        return false;
    };
    policy_statements(&parsed).iter().all(grants_to_an_account)
}

/// The principal value that names EVERY principal rather than one: the IAM wildcard.
pub(super) const WILDCARD_PRINCIPAL: &str = "*";

/// Whether an access policy ALLOWS every principal — a statement that allows the wildcard
/// principal and narrows it with no condition.
///
/// An access policy "authorizes cross-account users to deliver their log events to the
/// associated destination", so its statements have to identify who is authorized: either a
/// concrete principal, or the wildcard narrowed by a condition, which is how a grant to an
/// organization names its members (`PutDestinationPolicy$forceUpdate`, the
/// [`ORGANIZATION_PRINCIPAL_CONDITION_KEY`] form). A statement allowing the wildcard with
/// no condition at all identifies nobody, and the document is refused.
///
/// `Principal` is read in both of its IAM forms — the bare string and the map from
/// principal type to one value or a list of them — and only an `Allow` statement is
/// examined, since a `Deny` over the wildcard restricts rather than grants. A document
/// that is not JSON allows nothing: the handler refuses it before asking.
pub fn access_policy_allows_every_principal(document: &str) -> bool {
    let Ok(serde_json::Value::Object(members)) =
        serde_json::from_str::<serde_json::Value>(document)
    else {
        return false;
    };
    // "Statement" carries either one statement or a list of them.
    let statements: Vec<&serde_json::Value> = match members.get("Statement") {
        Some(serde_json::Value::Array(items)) => items.iter().collect(),
        Some(statement) => vec![statement],
        None => Vec::new(),
    };
    statements.into_iter().any(|statement| {
        let allows = statement
            .get("Effect")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|effect| effect.eq_ignore_ascii_case("Allow"));
        let narrowed = statement
            .get("Condition")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|condition| !condition.is_empty());
        allows && !narrowed && names_wildcard_principal(statement.get("Principal"))
    })
}

/// Whether a statement's `Principal` is the wildcard, in either of the forms IAM writes it:
/// the bare string, or a map from principal type to the wildcard (alone or among a list).
pub(super) fn names_wildcard_principal(principal: Option<&serde_json::Value>) -> bool {
    let is_wildcard =
        |value: &serde_json::Value| value.as_str() == Some(WILDCARD_PRINCIPAL);
    match principal {
        Some(serde_json::Value::Object(by_type)) => by_type.values().any(|named| match named {
            serde_json::Value::Array(items) => items.iter().any(is_wildcard),
            value => is_wildcard(value),
        }),
        Some(value) => is_wildcard(value),
        None => false,
    }
}

/// The IAM action registering a subscription filter is authorized as: this service's action
/// namespace (`logs`, its `arnNamespace`) and the operation's own name. It is the action a
/// destination's access policy has to authorize, since one "is used to authorize claims to
/// register a subscription filter against a given destination" (`PutDestinationPolicy`).
pub const PUT_SUBSCRIPTION_FILTER_ACTION: &str = "logs:PutSubscriptionFilter";

/// AWS's refusal for a call nothing authorizes: the account the request was signed for, the
/// action it asked for and the resource it asked for. Every variable part comes from the
/// request and from this store — the caller's account id, the fully qualified action and the
/// resource's own ARN — so the sentence states the rule rather than one call's values.
pub fn not_authorized_message(account_id: &str, action: &str, resource_arn: &str) -> String {
    format!(
        "User with accountId: {account_id} is not authorized to perform: {action} on \
         resource: {resource_arn}"
    )
}

/// Whether the calling principal may take `action` on the logical destination `destination`.
///
/// The destination's ACCESS POLICY is the only thing that authorizes one: "Through an access
/// policy, a destination controls what is written to it. By default, PutDestination does not
/// set any access policy with the destination, which means a cross-account user cannot call
/// PutSubscriptionFilter against this destination. To enable this, the destination owner must
/// call PutDestinationPolicy after PutDestination" (`PutDestination`). So a destination
/// carrying NO policy authorizes nobody — not even the account that owns it, which is what
/// AWS answers such a call with — and a destination carrying one is decided by the tree's
/// single evaluation of IAM documents ([`service_common::iam_authorization`]).
///
/// The destination's OWN account also reaches it through its identity-based policies, which
/// is why an attached policy naming only another account does not revoke the owner; an
/// explicit `Deny` about this request still refuses it, as IAM evaluates one.
pub fn destination_authorizes(destination: &DestinationRecord, action: &str) -> bool {
    let Some(document) = destination.access_policy.as_deref() else {
        return false;
    };
    // Every spelling an access policy can name the caller by: the principal this emulator's
    // credential speaks for, and the account that principal belongs to, which IAM lets a
    // resource policy name in place of the principal itself.
    let caller = ::service_common::caller_principal::ambient_user_arn();
    let mut principals = vec![caller.clone()];
    principals.extend(::service_common::iam_authorization::account_principals(
        &caller,
    ));
    let stated = ::service_common::iam_authorization::resource_decision(
        document,
        &::service_common::iam_authorization::Request::new(action, &destination.arn, &principals),
    );
    let by_identity = if arn_account_id(&destination.arn) == arn_account_id(&caller) {
        ::service_common::iam_authorization::Decision::Allowed
    } else {
        ::service_common::iam_authorization::Decision::NotStated
    };
    ::service_common::iam_authorization::is_allowed(by_identity, stated)
}

/// The refusal a request that would register a subscription filter against the destination
/// `destination_arn` names is answered with, or `None` when the destination authorizes it.
///
/// `None` as well for an ARN that names no logical destination THIS account holds: a
/// physical destination (a stream, a delivery stream, a function) carries no access policy,
/// and a destination of another account is a resource whose policy this store does not hold
/// — a refusal has to be grounded in a policy that was read, so neither is refused here.
pub fn subscription_destination_refusal(
    store: &ComAmazonawsCloudwatchlogsLogs20140328State,
    destination_arn: &str,
) -> Option<String> {
    let destination = store.destination_by_arn(destination_arn)?;
    (!destination_authorizes(destination, PUT_SUBSCRIPTION_FILTER_ACTION)).then(|| {
        not_authorized_message(
            ::engine::emulator_account_id(),
            PUT_SUBSCRIPTION_FILTER_ACTION,
            destination_arn,
        )
    })
}

/// The refusal CloudWatch Logs answers a `PutDestinationPolicy` that would replace a
/// destination's account-granting access policy with one granting an organization without the
/// `forceUpdate` affirmation that the senders' subscription filters have already been updated.
/// The message names the parameter the caller has to set, so one request carries the whole
/// rule; it says it per CLASS of update rather than per request value, so every such put is
/// refused identically.
pub const DESTINATION_POLICY_FORCE_UPDATE_REQUIRED: &str =
    "forceUpdate must be true to update an existing destination policy to grant permission to \
     an organization ID instead of individual accounts";
