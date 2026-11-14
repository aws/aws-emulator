// SPDX-License-Identifier: CC0-1.0

//! The SimpleDB domains this account holds, and the items inside them.
//!
//! This is the ONE registry a `domainName` is resolved against. Three things put a
//! domain in it: the operator's declaration ([`super::EXISTING_DOMAINS_ENV`]), a stored
//! export (an export can only have been started for a domain that existed), and a
//! `CreateDomain` on the Amazon SimpleDB 2009-04-15 query API this endpoint serves
//! ([`crate::service::sdb_query_api`]). The items that API writes live on the domain's
//! own record, so an export of a domain counts the items the domain actually holds and
//! a `DeleteDomain` takes them with it.
//!
//! An attribute is MULTI-VALUED: one name holds a set of values, which is why the value
//! side of an item is a set rather than a single string — `PutAttributes` without
//! `Replace` adds a value to the name it already has, and `GetAttributes` reports one
//! `<Attribute>` per value.
//!
//! Every bound below is either a figure Amazon SimpleDB publishes (named as such) or
//! this emulator's own ceiling (named as such): the store is this process's memory, so
//! the surface that writes it has to be finite.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use super::ComAmazonawsSimpledbv2Simpledbv2State;

/// The most domains one account holds — "you can create up to 250 domains per account",
/// the figure Amazon SimpleDB publishes for the quota its `NumberDomainsExceeded`
/// reports.
pub const MAX_DOMAINS: usize = 250;

/// The fewest characters a domain name may carry: Amazon SimpleDB documents "domain
/// names must be between 3 and 255 characters".
pub const MIN_DOMAIN_NAME_LEN: usize = 3;

/// The most characters a domain name may carry. See [`MIN_DOMAIN_NAME_LEN`].
pub const MAX_DOMAIN_NAME_LEN: usize = 255;

/// The most attribute name-value pairs ONE item holds: Amazon SimpleDB documents 256
/// name-value pairs per item, which is what its `NumberItemAttributesExceeded` reports.
pub const MAX_ITEM_ATTRIBUTES: usize = 256;

/// The most attributes one call may submit: Amazon SimpleDB documents 256 attribute
/// name-value pairs per `PutAttributes` call, which is what its
/// `NumberSubmittedAttributesExceeded` reports. It is also the most attribute names one
/// `GetAttributes` may ask for.
pub const MAX_SUBMITTED_ATTRIBUTES: usize = 256;

/// The most bytes an item name, an attribute name or an attribute value may carry:
/// Amazon SimpleDB documents 1024 bytes for each of the three.
pub const MAX_TEXT_BYTES: usize = 1024;

/// The most bytes of item names, attribute names and attribute values ONE domain holds
/// in this emulator.
///
/// NOT the published quota — Amazon SimpleDB publishes 10 GB and a billion attributes
/// per domain — so this is NOT a fidelity claim: the store is this process's memory, and
/// a caller looping on `PutAttributes` must not be able to grow it without end. It is
/// set well above any realistic script, and a write past it is refused with the
/// `NumberDomainBytesExceeded` the service publishes for a domain that cannot take more
/// bytes.
pub const MAX_DOMAIN_BYTES: usize = 256 * 1024;

/// The most domain names `ListDomains` returns in one page, and the page size it applies
/// when the request names no `MaxNumberOfDomains`: Amazon SimpleDB documents
/// `MaxNumberOfDomains` as 1..100 with a default of 100.
pub const MAX_LIST_DOMAINS: usize = 100;

/// The attributes of one item: each attribute name and the SET of values stored under
/// it.
pub type ItemAttributes = BTreeMap<String, BTreeSet<String>>;

/// One SimpleDB domain: the items it holds, and the instant its contents last changed.
#[derive(Debug, Clone)]
pub struct Domain {
    items: BTreeMap<String, ItemAttributes>,
    changed_at: SystemTime,
}

impl Domain {
    /// An empty domain, recorded as having last changed at `now` — the instant it was
    /// created, which is when its (empty) metadata was first true.
    pub(super) fn new(now: SystemTime) -> Self {
        Self {
            items: BTreeMap::new(),
            changed_at: now,
        }
    }

    /// The instant this domain's contents last changed: when it was created, or when the
    /// last write landed. It is what `DomainMetadata` reports as the moment the metadata
    /// it answers with was true, so a domain nothing has written to since reports the
    /// same instant on every read.
    pub fn changed_at(&self) -> SystemTime {
        self.changed_at
    }

    /// How many items this domain holds — the `ItemCount` of its metadata, and the count
    /// of items an export of this domain covers.
    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    /// The attributes stored under `item_name`, or `None` when the domain holds no item
    /// of that name. An item that holds no attribute is not stored at all, so an absent
    /// item and an empty one are the same thing here, exactly as they are on the wire.
    pub fn attributes_of(&self, item_name: &str) -> Option<&ItemAttributes> {
        self.items.get(item_name)
    }

    /// Every attribute NAME this domain uses, each once — the `AttributeNameCount` of its
    /// metadata is how many there are.
    pub fn attribute_names(&self) -> BTreeSet<&str> {
        self.items
            .values()
            .flat_map(|item| item.keys().map(String::as_str))
            .collect()
    }

    /// How many attribute name-value pairs this domain holds across every item — the
    /// `AttributeValueCount` of its metadata.
    pub fn attribute_value_count(&self) -> usize {
        self.items
            .values()
            .map(|item| item.values().map(BTreeSet::len).sum::<usize>())
            .sum()
    }

    /// The total size of this domain's item names, in bytes.
    pub fn item_names_size_bytes(&self) -> usize {
        self.items.keys().map(|name| name.len()).sum()
    }

    /// The total size of this domain's UNIQUE attribute names, in bytes.
    pub fn attribute_names_size_bytes(&self) -> usize {
        self.attribute_names().iter().map(|name| name.len()).sum()
    }

    /// The total size of this domain's attribute values, in bytes — every value of every
    /// attribute of every item.
    pub fn attribute_values_size_bytes(&self) -> usize {
        self.items
            .values()
            .map(|item| {
                item.values()
                    .flat_map(|values| values.iter().map(String::len))
                    .sum::<usize>()
            })
            .sum()
    }

    /// The bytes this domain occupies, as [`MAX_DOMAIN_BYTES`] counts them: its item
    /// names, its attribute names and its attribute values. One figure for the whole
    /// domain, so one ceiling bounds items, attributes and values together.
    pub fn stored_bytes(&self) -> usize {
        self.items
            .iter()
            .map(|(item_name, attributes)| item_bytes(item_name, attributes))
            .sum()
    }
}

/// The bytes one item occupies: its own name, and the name and value of each pair it
/// holds. The attribute names are counted per item rather than once per domain, which is
/// the conservative reading for a ceiling.
fn item_bytes(item_name: &str, attributes: &ItemAttributes) -> usize {
    let attribute_bytes: usize = attributes
        .iter()
        .map(|(name, values)| {
            values
                .iter()
                .map(|value| name.len() + value.len())
                .sum::<usize>()
        })
        .sum();
    item_name.len() + attribute_bytes
}

/// One `Attribute.N` of a `PutAttributes` call: the pair to store, and whether it
/// REPLACES the values the item already holds under that name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributeWrite {
    /// `Attribute.N.Name`.
    pub name: String,
    /// `Attribute.N.Value`.
    pub value: String,
    /// `Attribute.N.Replace`, which defaults to false — "if you specify Replace=true,
    /// the new value replaces the existing value", and otherwise the value is added to
    /// the ones already stored under that name.
    pub replace: bool,
}

/// One `Attribute.N` of a `DeleteAttributes` call: the attribute to delete from, and the
/// one value to delete under it — `None` deletes every value of that name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributeTarget {
    /// `Attribute.N.Name`.
    pub name: String,
    /// `Attribute.N.Value`, absent when the whole attribute goes.
    pub value: Option<String>,
}

/// Why the store refused a write. Each variant is one published refusal of the
/// 2009-04-15 API; the wire layer words them ([`crate::service::sdb_query_api`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteRefusal {
    /// The account holds no domain of that name.
    NoSuchDomain,
    /// The item would hold more than [`MAX_ITEM_ATTRIBUTES`] name-value pairs.
    ItemAttributesExceeded,
    /// The domain would hold more than [`MAX_DOMAIN_BYTES`] bytes.
    DomainBytesExceeded,
}

impl ComAmazonawsSimpledbv2Simpledbv2State {
    /// The domain `domain_name` names, or `None` when the account holds no such domain.
    pub fn domain(&self, domain_name: &str) -> Option<&Domain> {
        self.domains.get(domain_name)
    }

    /// How many domains the account holds.
    pub fn domain_count(&self) -> usize {
        self.domains.len()
    }

    /// Remove `domain_name` and every item it held, answering whether the account held
    /// it at all.
    ///
    /// "Deleting a domain that does not exist" changes nothing and is not an error, which
    /// is what makes the 2009-04-15 `DeleteDomain` a no-op on an absent name.
    pub fn delete_domain(&mut self, domain_name: &str) -> bool {
        self.domains.remove(domain_name).is_some()
    }

    /// The page of domain names that follows `after`, at most `limit` of them, in name
    /// order — the enumeration `ListDomains` pages through.
    ///
    /// The cursor is a NAME rather than a position, so a page is unaffected by a domain
    /// created or deleted between two calls: the next page is simply the names that sort
    /// after the last one reported.
    pub fn domain_names_after(&self, after: Option<&str>, limit: usize) -> Vec<&str> {
        self.domains
            .keys()
            .map(String::as_str)
            .filter(|name| after.is_none_or(|cursor| *name > cursor))
            .take(limit)
            .collect()
    }

    /// The `NextToken` naming the position after the domain `last_name`.
    ///
    /// The cursor travels inside the token, authenticated under this store's own secret
    /// ([`::service_common::page_tokens`]), and is rendered as hex because the token is
    /// opaque text on the wire. A caller cannot produce the authenticator for a cursor of
    /// its own, so the only positions this enumeration honours are the ones it offered.
    pub fn domain_page_token(&self, last_name: &str) -> String {
        ::hex::encode(self.page_token_key.mint(last_name))
    }

    /// The position `token` names, or `None` for any value this store did not mint.
    pub fn domain_page_cursor(&self, token: &str) -> Option<String> {
        self.page_token_key
            .cursor_of(&::hex::decode(token).ok()?)
    }

    /// Write `writes` into `item_name` of `domain_name`, as `PutAttributes` does.
    ///
    /// ALL OR NOTHING: the pairs are applied to a copy of the item and the copy is stored
    /// only once what it became is inside every bound, so a refused call leaves the item
    /// exactly as it was rather than half written.
    ///
    /// A `Replace` pair clears what the item held under that name ONCE per call, so two
    /// replacing pairs naming one attribute both end up stored — which is what the API's
    /// "the new value replaces the existing value" means for a name submitted twice.
    pub fn put_item_attributes(
        &mut self,
        domain_name: &str,
        item_name: &str,
        writes: &[AttributeWrite],
        now: SystemTime,
    ) -> Result<(), WriteRefusal> {
        let Some(domain) = self.domains.get_mut(domain_name) else {
            return Err(WriteRefusal::NoSuchDomain);
        };
        let mut item = domain.items.get(item_name).cloned().unwrap_or_default();
        let mut cleared: BTreeSet<&str> = BTreeSet::new();
        for write in writes {
            if write.replace && cleared.insert(write.name.as_str()) {
                item.remove(write.name.as_str());
            }
            item.entry(write.name.clone())
                .or_default()
                .insert(write.value.clone());
        }
        let pairs: usize = item.values().map(BTreeSet::len).sum();
        if pairs > MAX_ITEM_ATTRIBUTES {
            return Err(WriteRefusal::ItemAttributesExceeded);
        }
        let without_item = domain
            .stored_bytes()
            .saturating_sub(match domain.items.get(item_name) {
                Some(stored) => item_bytes(item_name, stored),
                None => 0,
            });
        if without_item.saturating_add(item_bytes(item_name, &item)) > MAX_DOMAIN_BYTES {
            return Err(WriteRefusal::DomainBytesExceeded);
        }
        domain.items.insert(item_name.to_string(), item);
        domain.changed_at = now;
        Ok(())
    }

    /// Delete `targets` from `item_name` of `domain_name`, as `DeleteAttributes` does: an
    /// empty `targets` deletes the whole item, a target with no value deletes every value
    /// of its attribute, and a target with a value deletes that one value.
    ///
    /// An attribute left with no values, and an item left with no attributes, are not
    /// stored: "if all attributes of an item are deleted, the item is deleted".
    pub fn delete_item_attributes(
        &mut self,
        domain_name: &str,
        item_name: &str,
        targets: &[AttributeTarget],
        now: SystemTime,
    ) -> Result<(), WriteRefusal> {
        let Some(domain) = self.domains.get_mut(domain_name) else {
            return Err(WriteRefusal::NoSuchDomain);
        };
        if targets.is_empty() {
            if domain.items.remove(item_name).is_some() {
                domain.changed_at = now;
            }
            return Ok(());
        }
        let Some(item) = domain.items.get_mut(item_name) else {
            // Deleting from an item the domain does not hold removes nothing and is not
            // an error: the state the caller asked for is the state it is already in.
            return Ok(());
        };
        let before = item.clone();
        for target in targets {
            match target.value.as_deref() {
                None => {
                    item.remove(target.name.as_str());
                }
                Some(value) => {
                    if let Some(values) = item.get_mut(target.name.as_str()) {
                        values.remove(value);
                        if values.is_empty() {
                            item.remove(target.name.as_str());
                        }
                    }
                }
            }
        }
        let changed = *item != before;
        if item.is_empty() {
            domain.items.remove(item_name);
        }
        if changed {
            domain.changed_at = now;
        }
        Ok(())
    }
}
