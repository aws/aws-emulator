// SPDX-License-Identifier: CC0-1.0

//! The ITEM actions of the Amazon SimpleDB 2009-04-15 API: `PutAttributes`,
//! `GetAttributes` and `DeleteAttributes`.
//!
//! All three are about ONE item of one domain, named by `DomainName` and `ItemName`, and
//! all three read and write the items on that domain's own record
//! ([`crate::state::simpledb_domains`]) — so what one writes is what the next reads, and
//! what they write is what an export of the domain counts.
//!
//! # An attribute holds a SET of values
//!
//! "Attributes can have multiple values": `PutAttributes` without `Replace` adds a value
//! to the name it already has, with `Replace` the pair replaces what that name held, and
//! `GetAttributes` reports one `<Attribute>` element per value. `DeleteAttributes`
//! deletes one value, a whole attribute, or the whole item, depending on how precisely
//! the request names what goes.
//!
//! # Conditional writes
//!
//! A write may carry `Expected.N.Name` with either `Expected.N.Value` (the attribute must
//! hold exactly that one value) or `Expected.N.Exists=false` (the attribute must not
//! exist). Every condition is evaluated BEFORE anything is written, so a refused call
//! leaves the item exactly as it was.

use aws_smithy_runtime_api::http::Response as RtResponse;

use super::envelope::{self, QueryApiError};
use super::parameters::{is_true, QueryParameters};
use super::{
    required_domain_name, required_item_name, within_text_bound, write_refusal, SimpleDbQueryApi,
};
use crate::state::simpledb_domains::{
    AttributeTarget, AttributeWrite, ItemAttributes, MAX_SUBMITTED_ATTRIBUTES,
};

/// The indexed parameter group carrying the attributes a call is about.
const ATTRIBUTE_GROUP: &str = "Attribute";

/// The indexed parameter group carrying the attribute names `GetAttributes` asks for.
const ATTRIBUTE_NAME_GROUP: &str = "AttributeName";

/// The indexed parameter group carrying a write's conditions.
const EXPECTED_GROUP: &str = "Expected";

/// The member of an indexed group naming the attribute.
const NAME_MEMBER: &str = "Name";

/// The member of an indexed group carrying the attribute's value.
const VALUE_MEMBER: &str = "Value";

/// The member of an `Attribute.N` group saying the pair replaces what the name held.
const REPLACE_MEMBER: &str = "Replace";

/// The member of an `Expected.N` group saying whether the attribute has to exist.
const EXISTS_MEMBER: &str = "Exists";

impl<Eng: Send + Sync + 'static> SimpleDbQueryApi<Eng> {
    /// `PutAttributes`: store the submitted pairs on one item of one domain.
    pub(super) fn put_attributes(
        &self,
        parameters: &QueryParameters,
        now: ::std::time::SystemTime,
        request_id: &str,
    ) -> Result<RtResponse, QueryApiError> {
        let domain_name = required_domain_name(parameters)?;
        let item_name = required_item_name(parameters)?;
        let writes = submitted_writes(parameters)?;
        let conditions = submitted_conditions(parameters)?;
        let store = self.store()?;
        let mut store = store.lock();
        // The domain is resolved, then the conditions are judged against what the item
        // holds NOW, and only then is anything written — so a refusal on either leaves
        // the store as it was.
        let held = store
            .domain(domain_name)
            .ok_or_else(envelope::no_such_domain)?
            .attributes_of(item_name);
        check_conditions(&conditions, held)?;
        store
            .put_item_attributes(domain_name, item_name, &writes, now)
            .map_err(write_refusal)?;
        Ok(envelope::response("PutAttributes", None, request_id))
    }

    /// `GetAttributes`: report one item's attribute name-value pairs.
    ///
    /// "If the item does not exist ... an empty set is returned", so an item the domain
    /// does not hold is an empty result rather than an error; only a domain the account
    /// does not hold is refused. `AttributeName.N` filters the report to the names asked
    /// for, and a request naming none asks for all of them.
    ///
    /// `ConsistentRead` is accepted and changes nothing: this store is the single copy
    /// every call of this emulator reads, so every read here is already the consistent
    /// one the parameter asks for.
    pub(super) fn get_attributes(
        &self,
        parameters: &QueryParameters,
        request_id: &str,
    ) -> Result<RtResponse, QueryApiError> {
        let domain_name = required_domain_name(parameters)?;
        let item_name = required_item_name(parameters)?;
        let requested: Vec<&str> = parameters.indexed_values(ATTRIBUTE_NAME_GROUP);
        if requested.len() > MAX_SUBMITTED_ATTRIBUTES {
            return Err(envelope::too_many_requested_attributes());
        }
        let store = self.store()?;
        let store = store.lock();
        let domain = store
            .domain(domain_name)
            .ok_or_else(envelope::no_such_domain)?;
        let mut result = String::new();
        if let Some(attributes) = domain.attributes_of(item_name) {
            for (name, values) in attributes {
                if !requested.is_empty() && !requested.contains(&name.as_str()) {
                    continue;
                }
                for value in values {
                    result.push_str(&format!(
                        "<{ATTRIBUTE_GROUP}>{}{}</{ATTRIBUTE_GROUP}>",
                        envelope::element(NAME_MEMBER, name),
                        envelope::element(VALUE_MEMBER, value),
                    ));
                }
            }
        }
        Ok(envelope::response(
            "GetAttributes",
            Some(result),
            request_id,
        ))
    }

    /// `DeleteAttributes`: delete pairs, whole attributes, or the whole item.
    ///
    /// "If all attributes of an item are deleted, the item is deleted", and a request
    /// naming no attribute at all deletes the item. Deleting what an item does not hold
    /// removes nothing and is not an error — the state the caller asked for is the state
    /// it is already in — so only a domain the account does not hold is refused.
    pub(super) fn delete_attributes(
        &self,
        parameters: &QueryParameters,
        now: ::std::time::SystemTime,
        request_id: &str,
    ) -> Result<RtResponse, QueryApiError> {
        let domain_name = required_domain_name(parameters)?;
        let item_name = required_item_name(parameters)?;
        let targets = submitted_targets(parameters)?;
        let conditions = submitted_conditions(parameters)?;
        let store = self.store()?;
        let mut store = store.lock();
        let held = store
            .domain(domain_name)
            .ok_or_else(envelope::no_such_domain)?
            .attributes_of(item_name);
        check_conditions(&conditions, held)?;
        store
            .delete_item_attributes(domain_name, item_name, &targets, now)
            .map_err(write_refusal)?;
        Ok(envelope::response("DeleteAttributes", None, request_id))
    }
}

/// The pairs a `PutAttributes` submitted.
///
/// A call has to submit at least one pair — there is nothing to write otherwise — and no
/// more than the API admits in one call. Each pair needs both its name and its value, and
/// each of the two is within the bytes the API admits.
fn submitted_writes(parameters: &QueryParameters) -> Result<Vec<AttributeWrite>, QueryApiError> {
    let groups = parameters.indexed_members(ATTRIBUTE_GROUP);
    if groups.is_empty() {
        return Err(envelope::missing_parameter(&format!(
            "{ATTRIBUTE_GROUP}.0.{NAME_MEMBER}"
        )));
    }
    if groups.len() > MAX_SUBMITTED_ATTRIBUTES {
        return Err(envelope::number_submitted_attributes_exceeded());
    }
    let mut writes = Vec::with_capacity(groups.len());
    for (index, group) in groups {
        let name = member(&group, index, NAME_MEMBER)?;
        let value = member(&group, index, VALUE_MEMBER)?;
        writes.push(AttributeWrite {
            name: name.to_string(),
            value: value.to_string(),
            replace: group.get(REPLACE_MEMBER).copied().is_some_and(is_true),
        });
    }
    Ok(writes)
}

/// The attributes a `DeleteAttributes` named: each with its name, and with the one value
/// to delete when it named one.
///
/// An empty list is a request to delete the whole item, which is this action's own
/// meaning for naming no attribute.
fn submitted_targets(parameters: &QueryParameters) -> Result<Vec<AttributeTarget>, QueryApiError> {
    let groups = parameters.indexed_members(ATTRIBUTE_GROUP);
    if groups.len() > MAX_SUBMITTED_ATTRIBUTES {
        return Err(envelope::number_submitted_attributes_exceeded());
    }
    let mut targets = Vec::with_capacity(groups.len());
    for (index, group) in groups {
        let name = member(&group, index, NAME_MEMBER)?;
        let value = match group.get(VALUE_MEMBER).copied() {
            Some(value) => {
                within_text_bound(
                    &format!("{ATTRIBUTE_GROUP}.{index}.{VALUE_MEMBER}"),
                    value,
                )?;
                Some(value.to_string())
            }
            None => None,
        };
        targets.push(AttributeTarget {
            name: name.to_string(),
            value,
        });
    }
    Ok(targets)
}

/// One member of an indexed attribute group, refused when the request left it out or
/// carried more text than the API admits. The refusal names the parameter the request
/// would have had to send, index and all.
fn member<'g>(
    group: &::std::collections::BTreeMap<&str, &'g str>,
    index: u32,
    member: &str,
) -> Result<&'g str, QueryApiError> {
    let parameter = format!("{ATTRIBUTE_GROUP}.{index}.{member}");
    let value = group
        .get(member)
        .copied()
        .ok_or_else(|| envelope::missing_parameter(&parameter))?;
    within_text_bound(&parameter, value)?;
    Ok(value)
}

/// One `Expected.N` condition a write has to satisfy.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Condition {
    /// `Expected.N.Name` + `Expected.N.Value`: the attribute holds exactly this one
    /// value.
    HoldsValue { name: String, value: String },
    /// `Expected.N.Name` + `Expected.N.Exists=false`: the item has no such attribute.
    Absent { name: String },
}

/// The conditions a write submitted.
///
/// The two forms are exclusive, which is what the API's own refusals say: a condition
/// that both names a value and expects the attribute to be absent is
/// `ExistsAndExpectedValue`, and one that expects it to exist without saying what it
/// holds is `IncompleteExpectedExpression`.
fn submitted_conditions(parameters: &QueryParameters) -> Result<Vec<Condition>, QueryApiError> {
    let groups = parameters.indexed_members(EXPECTED_GROUP);
    let mut conditions = Vec::with_capacity(groups.len());
    for (index, group) in groups {
        let name = group
            .get(NAME_MEMBER)
            .copied()
            .ok_or_else(|| envelope::missing_parameter(&format!(
                "{EXPECTED_GROUP}.{index}.{NAME_MEMBER}"
            )))?;
        let value = group.get(VALUE_MEMBER).copied();
        let exists = group.get(EXISTS_MEMBER).copied();
        conditions.push(match (value, exists) {
            (Some(_), Some(exists)) if !is_true(exists) => {
                return Err(envelope::exists_and_expected_value())
            }
            (None, Some(exists)) if !is_true(exists) => Condition::Absent {
                name: name.to_string(),
            },
            (None, _) => return Err(envelope::incomplete_expected_expression()),
            (Some(value), _) => Condition::HoldsValue {
                name: name.to_string(),
                value: value.to_string(),
            },
        });
    }
    Ok(conditions)
}

/// Judge every condition against what the item holds, refusing the FIRST one that does
/// not hold.
///
/// `held` is `None` for an item the domain does not hold, which holds no attribute at
/// all: an `Absent` condition is satisfied by it and a `HoldsValue` condition is not.
fn check_conditions(
    conditions: &[Condition],
    held: Option<&ItemAttributes>,
) -> Result<(), QueryApiError> {
    for condition in conditions {
        match condition {
            Condition::Absent { name } => {
                if held.is_some_and(|attributes| attributes.contains_key(name.as_str())) {
                    return Err(envelope::conditional_check_failed_exists(name));
                }
            }
            Condition::HoldsValue { name, value } => {
                let values = held
                    .and_then(|attributes| attributes.get(name.as_str()))
                    .ok_or_else(|| envelope::attribute_does_not_exist(name))?;
                let mut held_values = values.iter();
                let (Some(only), None) = (held_values.next(), held_values.next()) else {
                    return Err(envelope::multi_valued_attribute(name));
                };
                if only != value {
                    return Err(envelope::conditional_check_failed_value(name, only, value));
                }
            }
        }
    }
    Ok(())
}
