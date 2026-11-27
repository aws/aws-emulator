// SPDX-License-Identifier: CC0-1.0

//! SHARDING on an elastic cluster: `shardCollection`, the shard key such a cluster admits, and
//! the rule that every document written to a sharded collection carries that key.
//!
//! An elastic cluster distributes a collection's documents over its shards, and what decides
//! which shard a document belongs to is the collection's SHARD KEY. A client shards a
//! collection by sending `shardCollection` with the key to shard it on
//! (`sh.shardCollection("<database>.<collection>", { "<field>": "hashed" })`), and the key an
//! elastic cluster admits is exactly one field, hashed: hash-based sharding is the one
//! strategy such a cluster has, and a hash is computed over one field's value.
//!
//! On such a cluster that command CREATES the collection it shards — a collection is sharded
//! when it comes into being and not afterwards — so every way it can fail is a collection that
//! was not created: a key that does not name exactly one field, a key whose one field names an
//! order instead of the hash, and a namespace the cluster already holds. Each is refused with
//! the sentence this endpoint words for that failure, and no collection is left behind.
//!
//! The key is what every later write of that collection is judged against: a document carrying
//! no value at the key's path belongs to no shard, an upsert the key does not target reaches no
//! shard to insert into, and the key a stored document was distributed by cannot be changed
//! afterwards. A refused write of one of those is reported as a WRITE ERROR of the entry it is
//! about ([`super::sharded_writes`]), because the command itself was served. A collection
//! nobody sharded is not judged at all — a collection is unsharded until a client shards it,
//! and writes to it are ordinary writes.
//!
//! The keys live with the cluster's documents ([`super::ClusterStorage`]), because which
//! collections are sharded is part of what the cluster holds: they go when the cluster does,
//! and when the collection or its database is dropped.

use std::collections::BTreeMap;

use ::bson::{Bson, Document};
use ::service_com_amazonaws_docdb_amazonrdsv19::state::cluster_data_plane as document_engine;

use document_engine::query;

use super::refusal::{self, Refusal};

/// The value a key's field carries to name hash-based sharding, which is the one sharding
/// strategy an elastic cluster has.
pub const HASHED: &str = "hashed";

/// How many fields the key of a sharded collection has on an elastic cluster: one, because
/// the shard a document belongs to is the hash of one field's value.
pub const SHARD_KEY_FIELDS: usize = 1;

/// The code every refusal about a collection's shard key carries: the key the collection needs
/// was not found — neither in the key a `shardCollection` named, nor in a document a write
/// carried, nor in the filter an upsert would have been targeted by.
const SHARD_KEY_CODE: i32 = 61;

/// The code a command that would create a collection the cluster already holds is refused with.
const NAMESPACE_EXISTS_CODE: i32 = 48;

/// The code a write the cluster will not carry out as the write it was sent as is refused with.
const ILLEGAL_OPERATION_CODE: i32 = 20;

/// The shard key of each sharded collection of one cluster, by `<database>.<collection>`.
#[derive(Debug, Default)]
pub struct ShardKeys {
    sharded: BTreeMap<String, String>,
}

impl ShardKeys {
    /// The field the collection `namespace` is sharded on, or `None` for a collection nobody
    /// sharded.
    #[must_use]
    pub fn key_of(&self, namespace: &str) -> Option<&str> {
        self.sharded.get(namespace).map(String::as_str)
    }

    /// Record that `namespace` is sharded on `field`.
    pub fn shard(&mut self, namespace: &str, field: &str) {
        self.sharded
            .insert(namespace.to_string(), field.to_string());
    }

    /// Forget the collection `namespace`, which dropping that collection does.
    pub fn forget(&mut self, namespace: &str) {
        self.sharded.remove(namespace);
    }

    /// Forget every collection of `database`, which dropping that database does.
    pub fn forget_database(&mut self, database: &str) {
        let prefix = format!("{database}.");
        self.sharded
            .retain(|namespace, _field| !namespace.starts_with(&prefix));
    }

    /// How many collections are sharded, which is what bounds this map: one entry per
    /// collection the cluster's volume holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.sharded.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sharded.is_empty()
    }
}

/// The `<database>.<collection>` a command names, split into its two halves.
///
/// Both halves have to be there: a value that names no collection inside a database is not a
/// namespace at all, whatever else it is.
pub fn split_namespace(namespace: &str) -> Result<(String, String), Refusal> {
    let (database, collection) = namespace
        .split_once('.')
        .ok_or_else(|| refusal::invalid_namespace(namespace))?;
    if database.is_empty() || collection.is_empty() {
        return Err(refusal::invalid_namespace(namespace));
    }
    Ok((database.to_string(), collection.to_string()))
}

/// The ONE field a shard key names, for a key of the shape an elastic cluster admits — exactly
/// [`SHARD_KEY_FIELDS`] field whose value is [`HASHED`].
///
/// Every other key is refused: a key over two fields, a key naming an ascending or descending
/// order (which is range-based sharding, which such a cluster does not have), and a key that
/// is not a document at all.
pub fn hashed_field(key: &Document) -> Result<String, Refusal> {
    let mut fields = key.iter();
    let Some((field, value)) = fields.next() else {
        return Err(shard_key_is_not_one_field());
    };
    // A key over two fields, and one whose single field is named by nothing at all, name no one
    // field to hash.
    if fields.next().is_some() || field.is_empty() {
        return Err(shard_key_is_not_one_field());
    }
    if !matches!(value, Bson::String(strategy) if strategy == HASHED) {
        return Err(shard_key_is_not_hashed());
    }
    // The path is bounded exactly as every other path this engine walks is.
    query::path_segments(field).map_err(Refusal::from)?;
    Ok(field.clone())
}

/// `shardCollection`: CREATE the collection the command names, sharded on the key it carries,
/// and answer as the engine does — the namespace that is now sharded.
///
/// The collection comes into being here, which is what sharding a collection on such a cluster
/// is, and the key is recorded only once it is there: a refused create (the cluster's own
/// collection or database ceiling) leaves nothing sharded.
///
/// A namespace the cluster ALREADY holds is a create of a collection that already exists —
/// whether a client sharded it, with this key or another one, or an ordinary write brought it
/// into being — because the documents such a collection holds were distributed by the key it
/// was created with.
pub fn shard_collection(
    volume: &mut document_engine::documents::Volume,
    keys: &mut ShardKeys,
    command: &Document,
    argument: &Bson,
) -> Result<Document, Refusal> {
    let namespace = match argument {
        Bson::String(namespace) => namespace.clone(),
        _ => {
            return Err(refusal::failed_to_parse(
                "shardCollection needs the namespace to shard",
            ))
        }
    };
    let (database, collection) = split_namespace(&namespace)?;
    let Some(Bson::Document(key)) = command.get("key") else {
        return Err(refusal::failed_to_parse("shardCollection needs a key"));
    };
    let field = hashed_field(key)?;
    if keys.key_of(&namespace).is_some() || volume.collection(&database, &collection).is_some() {
        return Err(collection_already_exists(&namespace));
    }
    volume
        .create_collection(&database, &collection)
        .map_err(Refusal::from)?;
    keys.shard(&namespace, &field);
    Ok(::bson::doc! { "collectionsharded": namespace })
}

/// Whether ONE document a write would store in `namespace` carries that collection's shard key.
///
/// A collection nobody sharded is not judged: the write is an ordinary one. A sharded
/// collection's documents each belong to the shard its key hashes to, so a document carrying no
/// value at that key's path belongs to no shard and is refused — that document, as the write
/// error of the entry it is, and not the whole command.
pub fn document_carries_the_shard_key(
    keys: &ShardKeys,
    namespace: &str,
    value: &Bson,
) -> Result<(), Refusal> {
    let Some(field) = keys.key_of(namespace) else {
        return Ok(());
    };
    // A value that is not a document at all is the shared command layer's to refuse, as it is
    // for an unsharded collection.
    let Bson::Document(document) = value else {
        return Ok(());
    };
    if query::value_at(document, field).is_none() {
        return Err(insert_without_the_shard_key());
    }
    Ok(())
}

/// Whether ONE statement of an `update` of `namespace` holds to that collection's shard key.
///
/// Two rules, and a statement of a collection nobody sharded is judged by neither:
///
/// * a statement with `upsert: true` inserts a document when its filter selects none, and the
///   shard that document goes to is read off the filter — so the filter has to PIN the shard
///   key by equality, and a statement whose filter does not is targeted at no shard at all;
/// * the shard a stored document belongs to is the hash of its shard key, so altering that key
///   moves the document from one shard to another: the cluster carries that out only for a
///   write it can REPLAY — a retryable write, or a statement of a multi-statement transaction,
///   which is what `retryable` says the command is — and refuses it outside one. A replacement
///   that would leave the document without the key is such a change too.
pub fn statement_respects_the_shard_key(
    keys: &ShardKeys,
    namespace: &str,
    upsert: bool,
    retryable: bool,
    filter: &Document,
    update: &Document,
) -> Result<(), Refusal> {
    let Some(field) = keys.key_of(namespace) else {
        return Ok(());
    };
    if upsert && !filter_pins(filter, field)? {
        return Err(upsert_is_not_targetable());
    }
    if !retryable && changes_the_shard_key(update, field) {
        return Err(shard_key_update_needs_a_retryable_write());
    }
    Ok(())
}

/// Whether `filter` pins the value at `field` by EQUALITY, which is what an upsert of a sharded
/// collection is targeted by.
///
/// Asked of the equalities the document engine itself reads out of a filter when it builds the
/// document an upsert would insert ([`query::upserted_document`]): handed a change that modifies
/// nothing, what it answers is exactly those equalities.
fn filter_pins(filter: &Document, field: &str) -> Result<bool, Refusal> {
    let pinned =
        query::upserted_document(filter, &::bson::doc! { "$set": {} }).map_err(Refusal::from)?;
    Ok(query::value_at(&pinned, field).is_some())
}

/// Whether the change `update` carries would alter the value at `field` of the document it is
/// applied to.
///
/// A change made of modifiers alters the field when one of them NAMES it — the path itself, a
/// path inside it or the path it is inside, because each of those is the same stored value; a
/// `$rename` names a second path in its argument's value. A change made of plain fields
/// REPLACES the whole document, so one carrying no value at the path would leave the document
/// without the key it is distributed by.
fn changes_the_shard_key(update: &Document, field: &str) -> bool {
    if !update.keys().any(|modifier| modifier.starts_with('$')) {
        return query::value_at(update, field).is_none();
    }
    for (modifier, argument) in update {
        // `$setOnInsert` applies only to the document an upsert INSERTS, which is judged as a
        // new document of the collection and not as a change to a stored one.
        if modifier == "$setOnInsert" {
            continue;
        }
        let Bson::Document(fields) = argument else {
            continue;
        };
        for (path, value) in fields {
            if names_the_same_value(path, field) {
                return true;
            }
            if modifier == "$rename" {
                if let Bson::String(to) = value {
                    if names_the_same_value(to, field) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Whether two field paths name the same stored value: the same path, or one inside the other.
fn names_the_same_value(path: &str, field: &str) -> bool {
    path == field
        || path
            .strip_prefix(field)
            .is_some_and(|inside| inside.starts_with('.'))
        || field
            .strip_prefix(path)
            .is_some_and(|inside| inside.starts_with('.'))
}

/// A `shardCollection` whose key does not name exactly [`SHARD_KEY_FIELDS`] field.
///
/// The sentence is this endpoint's own and is FIXED text: it carries no part of the request,
/// and it says what failed — the collection was not created — rather than naming a key that
/// was not found, which is what the numeric code alone would read as. It carries no `codeName`
/// either, as this endpoint's own refusals do not.
#[must_use]
fn shard_key_is_not_one_field() -> Refusal {
    Refusal {
        code: SHARD_KEY_CODE,
        code_name: None,
        message: "Create collection failed. Must have exactly one Shard Key".to_string(),
    }
}

/// A `shardCollection` whose one field names an order — range-based sharding, which such a
/// cluster does not have — instead of [`HASHED`]. Fixed text, worded as
/// [`shard_key_is_not_one_field`] is.
#[must_use]
fn shard_key_is_not_hashed() -> Refusal {
    Refusal {
        code: SHARD_KEY_CODE,
        code_name: None,
        message: "Create collection failed. Shard Key type must be hashed not range based"
            .to_string(),
    }
}

/// A `shardCollection` of a namespace the cluster already holds, which is a create of a
/// collection that already exists. The namespace is the one the command named, and nothing else
/// of the request is in the sentence.
#[must_use]
fn collection_already_exists(namespace: &str) -> Refusal {
    Refusal {
        code: NAMESPACE_EXISTS_CODE,
        code_name: None,
        message: format!("a collection '{namespace}' already exists"),
    }
}

/// A document a write would store in a sharded collection carrying no value at its shard key.
///
/// Fixed text, as every refusal this endpoint words itself is: it carries no part of the
/// request — not the field the collection is sharded on and nothing of the document — and no
/// `codeName` beside the code.
#[must_use]
fn insert_without_the_shard_key() -> Refusal {
    Refusal {
        code: SHARD_KEY_CODE,
        code_name: None,
        message: "Insert for sharded collection must have shardKey".to_string(),
    }
}

/// An upserting statement whose filter pins no shard key, so the document it would insert
/// belongs to no shard the cluster could send it to. Fixed text, carrying no `codeName`, as
/// this endpoint's own refusals do not.
#[must_use]
fn upsert_is_not_targetable() -> Refusal {
    Refusal {
        code: SHARD_KEY_CODE,
        code_name: None,
        message: "Failed to target upsert by query :: could not extract exact shard key"
            .to_string(),
    }
}

/// An update that would alter the shard key of a document the collection holds, sent as a write
/// the cluster cannot replay.
///
/// Fixed text, naming what the client has to send instead: the write has to be one the cluster
/// can carry out again, because moving a document between shards is a write on each of them.
#[must_use]
fn shard_key_update_needs_a_retryable_write() -> Refusal {
    Refusal {
        code: ILLEGAL_OPERATION_CODE,
        code_name: None,
        message: "Must run update to shard key field in a multi-statement transaction or with \
                  retryWrites: true."
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of the statement table: the case, the namespace, whether the statement upserts,
    /// its filter, its change, and the code it is refused with.
    type StatementRow = (
        &'static str,
        &'static str,
        bool,
        Document,
        Document,
        Option<i32>,
    );

    /// RULE: the shard key an elastic cluster admits is exactly one field whose value is
    /// `hashed`; a key naming any other number of fields, and a key whose one field names an
    /// order instead, is refused as the create of a collection that did not happen.
    ///
    /// The rows span what the rule generalises over: how many fields the key names, what
    /// strategy each names (the hashed one, a range order in either direction, a type that is
    /// no strategy at all) and the field's own form (a plain name, a path). The expectation is
    /// the field the key names, or the sentence the refusal carries.
    #[test]
    fn the_shard_key_is_one_hashed_field() {
        let one_field = "Create collection failed. Must have exactly one Shard Key";
        let hashed = "Create collection failed. Shard Key type must be hashed not range based";
        let rows: Vec<(&str, Document, Result<&str, &str>)> = vec![
            ("one hashed field", ::bson::doc! { "tenant": HASHED }, Ok("tenant")),
            (
                "one hashed field named by a path",
                ::bson::doc! { "owner.tenant": HASHED },
                Ok("owner.tenant"),
            ),
            (
                "an ascending field",
                ::bson::doc! { "tenant": 1_i32 },
                Err(hashed),
            ),
            (
                "a descending field",
                ::bson::doc! { "tenant": -1_i32 },
                Err(hashed),
            ),
            (
                "two hashed fields",
                ::bson::doc! { "tenant": HASHED, "region": HASHED },
                Err(one_field),
            ),
            (
                "a hashed field beside a range one",
                ::bson::doc! { "tenant": HASHED, "region": 1_i32 },
                Err(one_field),
            ),
            ("no field at all", Document::new(), Err(one_field)),
            (
                "a field named by nothing",
                ::bson::doc! { "": HASHED },
                Err(one_field),
            ),
            (
                "a strategy that is not a string",
                ::bson::doc! { "tenant": true },
                Err(hashed),
            ),
            (
                "a strategy spelled otherwise",
                ::bson::doc! { "tenant": "HASHED" },
                Err(hashed),
            ),
        ];
        for (case, key, expected) in rows {
            let read = hashed_field(&key);
            match expected {
                Ok(field) => assert_eq!(
                    read.as_deref().ok(),
                    Some(field),
                    "case {case}: expected the key to name {field}, got {read:?}"
                ),
                Err(sentence) => {
                    let refused = read.clone().err();
                    assert_eq!(
                        refused.as_ref().map(|refusal| refusal.message.as_str()),
                        Some(sentence),
                        "case {case}: expected the refusal to say {sentence:?}, got {read:?}"
                    );
                    // The refusal carries the code alone: no name is published beside it.
                    assert_eq!(
                        refused.map(|refusal| (refusal.code, refusal.code_name)),
                        Some((SHARD_KEY_CODE, None)),
                        "case {case}: expected the shard key code and no name, got {read:?}"
                    );
                }
            }
        }
    }

    /// RULE: a document stored in a SHARDED collection carries a value at that collection's
    /// shard key; a collection nobody sharded is not judged.
    ///
    /// The rows span the key's form (a plain field, a path), whether the document carries it,
    /// and whether the collection is sharded at all.
    #[test]
    fn every_document_of_a_sharded_write_carries_the_key() {
        let mut keys = ShardKeys::default();
        keys.shard("shop.orders", "tenant");
        keys.shard("shop.events", "owner.tenant");
        let rows: Vec<(&str, &str, Document, bool)> = vec![
            (
                "a document carrying the key",
                "shop.orders",
                ::bson::doc! { "tenant": "acme", "total": 2_i32 },
                true,
            ),
            (
                "a document missing the key",
                "shop.orders",
                ::bson::doc! { "total": 1_i32 },
                false,
            ),
            (
                "a key named by a path, carried",
                "shop.events",
                ::bson::doc! { "owner": { "tenant": "acme" } },
                true,
            ),
            (
                "a key named by a path, missing",
                "shop.events",
                ::bson::doc! { "owner": { "region": "eu" } },
                false,
            ),
            (
                "a collection nobody sharded",
                "shop.carts",
                ::bson::doc! { "total": 1_i32 },
                true,
            ),
        ];
        for (case, namespace, document, expected) in rows {
            let decided =
                document_carries_the_shard_key(&keys, namespace, &Bson::Document(document));
            assert_eq!(
                decided.is_ok(),
                expected,
                "case {case}: expected the document to be admitted {expected}, got {decided:?}"
            );
        }
    }

    /// RULE: an upserting statement of a sharded collection is targeted by the shard key its
    /// FILTER pins, and a statement altering the shard key of a document the collection holds
    /// is carried out only as a write the cluster can replay.
    ///
    /// The rows span what the rule generalises over: whether the statement upserts, where the
    /// key value would come from (the filter, by a plain equality or an `$eq`, the change, or
    /// nowhere), the form of the change (modifiers, a `$rename`, a whole replacement), whether
    /// it names the key, a path inside it or none, whether the write can be replayed, and
    /// whether the collection is sharded at all.
    #[test]
    fn a_statement_targets_by_the_key_and_moves_it_only_on_a_retryable_write() {
        let mut keys = ShardKeys::default();
        keys.shard("shop.orders", "tenant");
        keys.shard("shop.events", "owner.tenant");
        let targeting = Some(SHARD_KEY_CODE);
        let immutable = Some(ILLEGAL_OPERATION_CODE);
        let rows: Vec<StatementRow> = vec![
            (
                "an ordinary change of another field",
                "shop.orders",
                false,
                ::bson::doc! { "tenant": "acme" },
                ::bson::doc! { "$set": { "total": 1_i32 } },
                None,
            ),
            (
                "an upsert whose filter pins the key",
                "shop.orders",
                true,
                ::bson::doc! { "tenant": "acme" },
                ::bson::doc! { "$set": { "total": 1_i32 } },
                None,
            ),
            (
                "an upsert whose filter pins the key with $eq",
                "shop.orders",
                true,
                ::bson::doc! { "tenant": { "$eq": "acme" } },
                ::bson::doc! { "$set": { "total": 1_i32 } },
                None,
            ),
            (
                "an upsert whose filter pins the key by a range",
                "shop.orders",
                true,
                ::bson::doc! { "tenant": { "$gt": "acme" } },
                ::bson::doc! { "$set": { "total": 1_i32 } },
                targeting,
            ),
            (
                "an upsert whose change sets the key but whose filter does not pin it",
                "shop.orders",
                true,
                ::bson::doc! { "total": 1_i32 },
                ::bson::doc! { "$setOnInsert": { "tenant": "acme" } },
                targeting,
            ),
            (
                "an upsert pinning no key at all",
                "shop.orders",
                true,
                ::bson::doc! { "total": 1_i32 },
                ::bson::doc! { "$set": { "region": "eu" } },
                targeting,
            ),
            (
                "a change of the key itself",
                "shop.orders",
                false,
                ::bson::doc! { "tenant": "acme" },
                ::bson::doc! { "$set": { "tenant": "globex" } },
                immutable,
            ),
            (
                "a change unsetting the key",
                "shop.orders",
                false,
                ::bson::doc! { "tenant": "acme" },
                ::bson::doc! { "$unset": { "tenant": "" } },
                immutable,
            ),
            (
                "a change renaming another field onto the key",
                "shop.orders",
                false,
                ::bson::doc! { "tenant": "acme" },
                ::bson::doc! { "$rename": { "owner": "tenant" } },
                immutable,
            ),
            (
                "a change of the document the key is inside",
                "shop.events",
                false,
                ::bson::doc! { "owner.tenant": "acme" },
                ::bson::doc! { "$set": { "owner": { "tenant": "acme" } } },
                immutable,
            ),
            (
                "a replacement carrying the key",
                "shop.orders",
                false,
                ::bson::doc! { "tenant": "acme" },
                ::bson::doc! { "tenant": "acme", "total": 2_i32 },
                None,
            ),
            (
                "a replacement dropping the key",
                "shop.orders",
                false,
                ::bson::doc! { "tenant": "acme" },
                ::bson::doc! { "total": 2_i32 },
                immutable,
            ),
            (
                "an upsert of a collection nobody sharded",
                "shop.carts",
                true,
                ::bson::doc! { "total": 1_i32 },
                ::bson::doc! { "$set": { "tenant": "acme" } },
                None,
            ),
        ];
        // Every row above is a write the cluster cannot replay, which is what a client sends
        // unless it asked for retryable writes or opened a transaction.
        for (case, namespace, upsert, filter, update, expected) in rows {
            let decided = statement_respects_the_shard_key(
                &keys, namespace, upsert, false, &filter, &update,
            );
            let code = decided.as_ref().err().map(|refusal| refusal.code);
            assert_eq!(
                code, expected,
                "case {case}: expected the statement to be refused with {expected:?}, got \
                 {decided:?}"
            );
        }
        // The same changes, sent as a write the cluster CAN replay: the shard key may move.
        let replayable: Vec<(&str, Document)> = vec![
            ("a change of the key itself", ::bson::doc! { "$set": { "tenant": "globex" } }),
            ("a change unsetting the key", ::bson::doc! { "$unset": { "tenant": "" } }),
            ("a replacement dropping the key", ::bson::doc! { "total": 2_i32 }),
        ];
        for (case, update) in replayable {
            let decided = statement_respects_the_shard_key(
                &keys,
                "shop.orders",
                false,
                true,
                &::bson::doc! { "tenant": "acme" },
                &update,
            );
            assert!(
                decided.is_ok(),
                "case {case}: expected a retryable write to be admitted, got {decided:?}"
            );
        }
    }
}
