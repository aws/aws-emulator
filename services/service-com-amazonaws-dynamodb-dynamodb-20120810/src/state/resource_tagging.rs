// SPDX-License-Identifier: CC0-1.0

//! How the TAGGING surface reads the `ResourceArn` that is its whole selector.
//!
//! One file for the resource an ARN-addressed operation (`TagResource`, `UntagResource`,
//! `ListTagsOfResource`) acts on: which of a table's resources the ARN names, the two
//! parts the value is held to, and the sentences each refusal states. It is about the
//! IDENTIFIER the request carries rather than about what the store holds, so it lives
//! beside [`super`]; `super` re-exports it, so every caller still names it
//! `crate::state::…`.

use super::*;

/// The DynamoDB resource an ARN-addressed operation's `ResourceArn` names: the region
/// it lives in, the table it belongs to, which of that table's resources it IS, and the
/// ARN as the request spelled it.
///
/// Such a call carries no `TableName`, so the ARN is the caller's whole selector — and
/// it is also the only identifier the request holds, which is why it is kept here: a
/// refusal about this resource echoes `arn`, never the `table` derived from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggedResource {
    pub region: String,
    pub table: String,
    pub arn: String,
    /// Whether the ARN addresses the table itself or that table's STREAM, which is what
    /// decides WHICH tag set the call reads and writes ([`Table::tag_set`]).
    pub target: PolicyTarget,
}

impl TaggedResource {
    /// The stream LABEL this ARN carries, for a resource that addresses a stream: the
    /// part after `/stream/`, which names one stream of a table among the ones it has
    /// published. `None` for a resource that addresses the table itself.
    pub fn stream_label(&self) -> Option<&str> {
        match self.target {
            PolicyTarget::Table => None,
            PolicyTarget::Stream => self.arn.split_once("/stream/").map(|(_, label)| label),
        }
    }
}

/// Resolve an ARN-addressed operation's `ResourceArn` to the resource it names, in the
/// scope the request itself is made in.
///
/// The one place an ARN is classified for the TAGGING surface, so `TagResource`,
/// `UntagResource` and `ListTagsOfResource` share a single rule, in two parts:
///
/// * SHAPE — the value must be an ARN of THIS service naming a resource a tag set is
///   held for: a table (`arn:<partition>:dynamodb:<region>:<account>:table/<name>`) or
///   that table's stream (`…:table/<name>/stream/<label>`), which is a resource of its
///   own wherever a `ResourceArn` is taken and carries its own tags. Any other
///   sub-resource ARN (`…/index/<i>`) names nothing this surface keeps tags for, so it
///   is refused rather than silently taken for its table;
/// * SCOPE — the ARN must address a resource of the endpoint the request reached: the
///   same partition, account and REGION this request is signed for, which is the one
///   this service answers for. [`table_arn`] mints exactly that form for a table of
///   this region, so the comparison against it is the whole scope check — the stream
///   suffix is this service's own and is read off before it.
///
/// Either way the value names no resource whose existence could be looked up, so it is
/// an INPUT failure ([`invalid_resource_arn`], 400 `ValidationException`) and not a
/// missing resource — an out-of-scope ARN is not "absent here", it is unaddressable
/// from here. Only a well-formed, in-scope ARN reaches the lookup, where a resource that
/// is not there is [`resource_arn_not_found`].
pub fn resolve_resource_arn(
    resource_arn: &str,
    request_region: &str,
) -> Result<TaggedResource, RequestError> {
    let invalid = || invalid_resource_arn(resource_arn);
    // `arn`, partition, service, region, account, resource — the resource field is
    // the last one and may itself carry `/`, so the split stops at six.
    let fields: Vec<&str> = resource_arn.splitn(6, ':').collect();
    let (partition, service, region, resource) = match fields.as_slice() {
        ["arn", partition, service, region, _account, resource] => {
            (*partition, *service, *region, *resource)
        }
        _ => return Err(invalid()),
    };
    if partition.is_empty() || service != "dynamodb" || region.is_empty() {
        return Err(invalid());
    }
    let path = resource.strip_prefix("table/").ok_or_else(invalid)?;
    // Which of the table's resources the ARN names, read off its resource field the way
    // the resource-policy surface reads the same two ([`resolve_policy_resource_arn`]): a
    // label is minted by this service, so what it has to be is non-empty; WHICH stream it
    // names is the lookup's question rather than the shape's.
    let (name, target) = match path.split_once('/') {
        None => (path, PolicyTarget::Table),
        Some((name, sub)) => match sub.strip_prefix("stream/") {
            Some(label) if !label.is_empty() && !label.contains('/') => {
                (name, PolicyTarget::Stream)
            }
            _ => return Err(invalid()),
        },
    };
    let table = resolve_table_name(name).map_err(|_| invalid())?;
    // The scope half of the rule, decided over the TABLE ARN this endpoint would mint:
    // anything but that form — another partition, another account, another region — is
    // unaddressable here, whatever this store happens to hold.
    let addressed = table_arn(request_region, &table);
    let scoped = match target {
        PolicyTarget::Table => resource_arn == addressed,
        PolicyTarget::Stream => resource_arn.starts_with(&format!("{addressed}/stream/")),
    };
    if !scoped {
        return Err(invalid());
    }
    Ok(TaggedResource {
        region: request_region.to_string(),
        table,
        arn: resource_arn.to_string(),
        target,
    })
}

/// The `ResourceNotFoundException` message an ARN-ADDRESSED operation answers with.
///
/// The tagging surface and the resource-policy surface carry no `TableName`: the
/// caller's whole selector is the `ResourceArn` member, so the refusal names THAT
/// member and echoes the value the request carried — the same construction the
/// name-addressed `not_found` makes for `Table`, over the identifier that was
/// actually sent. Nothing derived from the ARN (the table name inside it, the stream
/// label) appears, because no member of the request holds it.
pub fn resource_arn_not_found(resource_arn: &str) -> RequestError {
    RequestError::NotFound(format!(
        "Requested resource not found: ResourceArn: {resource_arn} not found"
    ))
}

/// The `ValidationException` message an ARN-ADDRESSED operation answers a
/// `ResourceArn` it cannot address with, in the form that states the rule the value
/// broke.
///
/// This is an INPUT refusal, not a lookup miss: a value that is not an ARN this
/// service can address names no resource whose existence could be checked. Two rules
/// can be broken, and the message states the one that was:
///
/// * a value that is NOT AN ARN AT ALL — it does not begin with the `arn:` every ARN
///   begins with — is refused by that rule, with the value the request carried, so a
///   caller that sent a bare resource name is told what the member takes;
/// * an ARN that is well formed but addresses no resource of this endpoint (another
///   service, partition, account or region, or a sub-resource no tag set is held for) is
///   refused by the member it was sent in, because the value IS an ARN and the rule it
///   broke is which resource it names.
pub fn invalid_resource_arn(resource_arn: &str) -> RequestError {
    RequestError::Validation(invalid_resource_arn_message(resource_arn))
}

/// The MESSAGE of that refusal, for the one caller that states it outside a handler: the
/// member's own bounds are declared by the model, so the validating walk can reach the
/// value before any handler does, and `src/service.rs` restates the refusal from here
/// rather than letting the walk's clause stand (see its `SELF_CHECKED_MEMBER_OPERATIONS`).
pub fn invalid_resource_arn_message(resource_arn: &str) -> String {
    if resource_arn.starts_with(ARN_PREFIX) {
        format!("{INVALID_TABLE_ARN}: Invalid ResourceArn provided as input {resource_arn}")
    } else {
        format!(
            "One or more parameter values were invalid: ARNs must start with '{ARN_PREFIX}': \
             {resource_arn}"
        )
    }
}

/// The CLASS sentence a `ResourceArn` the TAGGING surface cannot read as an ARN of a
/// resource of this endpoint is refused by, and the WHOLE message when there is no value
/// to name.
///
/// The refusal states the failure in two parts: this sentence, which is the rule that was
/// broken, and — when the request carried a value — the detail clause naming it
/// ([`invalid_resource_arn`]). A request that carries NO value for the member leaves
/// nothing to detail, so the class sentence is the whole message: fixed text, no member
/// path, nothing echoed. `src/service.rs` is where the member's absence reaches it, because
/// the walk is what finds a member missing.
pub const INVALID_TABLE_ARN: &str = "Invalid TableArn";

/// The member path the validating walk reports an ARN-ADDRESSED operation's selector
/// under: the lowerCamel spelling of `ResourceArn`, which is the path a clause about that
/// member names it by.
pub const RESOURCE_ARN_MEMBER_PATH: &str = "resourceArn";

/// The `ValidationException` message a tagging call answers a tag VALUE AWS does not accept
/// with: the class sentence, over the value as the request spelled it.
///
/// DynamoDB's own sentence for the class, not a constraint clause — no `<N> validation
/// error(s) detected: ` envelope, no member path, no bound — stated the same way the
/// reserved-namespace refusal of a tag KEY is (`super::reserved_tag_prefix`): the fact is
/// about the tag the request carried rather than about the member it travelled in. It
/// answers whatever is wrong with the value, the maximum the model declares on it included
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#TagValueString")` gives
/// `@length` 0..256), so `src/service.rs` restates a violation the validating walk reports
/// against the member from here rather than letting that walk's clause stand.
pub fn invalid_tag_value_message(value: &str) -> String {
    format!("The Tag Value provided is invalid, Value: {value}")
}

/// The same for a tag KEY AWS does not accept — the bound the model declares on it
/// included (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#TagKeyString")`
/// gives `@length` 1..128, so the EMPTY key a removal states is refused by this sentence).
pub fn invalid_tag_key_message(key: &str) -> String {
    format!("The Tag Key provided is invalid, Key: {key}")
}
