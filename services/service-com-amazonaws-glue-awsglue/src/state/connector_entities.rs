// SPDX-License-Identifier: CC0-1.0

//! The CONNECTOR behind one connection: what the data source that connection points at
//! holds, which is what this service's entity reads report.
//!
//! `ListEntities` "[r]eturns the available entities supported by the connection type", so an
//! entity is an object of the SOURCE — a SaaS object, a stream, a table — and never a record
//! of this service's own. What can be reported about one is therefore decided by whether that
//! source is served HERE:
//!
//! * a connection whose source is another service of this same emulator is read through that
//!   service's OWN listing, over the in-process callback, exactly as a crawl of a
//!   `DynamoDBTarget` reads the table behind it ([`super::dynamodb_crawl_source`]) — so the
//!   entities reported are the objects that service answers for, and nothing else; and
//! * a connection whose source is outside this emulator — a SaaS tenant, a database host, a
//!   record stream — has no connector here at all, so the read reports the federation failure
//!   it is rather than entities nothing enumerated. This is the same reason a connection TEST
//!   reaches no data source ([`super::connection_tests`]).
//!
//! The objects are read ONE PAGE at a time ([`ENTITIES_PER_PAGE`]), resuming from the
//! position the previous page stopped at: the source's own listing carries that position, so
//! one page of an entity read costs one page of its listing and none of the source is
//! buffered here.

use ::aws_smithy_types::error::display::DisplayErrorContext;

use super::*;
use crate::ops::types::Entity;

/// How many entities ONE page of an entity read reports.
///
/// CHOSEN: `ListEntities` models no `MaxResults`, so how much of a source one segment
/// carries is the service's own choice. It is also the largest page Amazon DynamoDB's own
/// table listing serves (`ListTables$Limit`, range 1..100), so one page of an entity read is
/// one page of the source's listing.
pub const ENTITIES_PER_PAGE: i32 = 100;

/// The `Category` of an entity that is a TABLE of its source: "[t]he type of entities that
/// are present in the response. This value depends on the source connection. For example
/// this is `SObjects` for Salesforce and `databases` or `schemas` or `tables` for sources
/// like Amazon Redshift" (`Entity$Category`).
pub const ENTITY_CATEGORY_TABLES: &str = "tables";

/// The `FederationSourceErrorCode` of a connection whose source this emulator serves no
/// connector for: there is no entity read of that source to make here, which is what this
/// value of the modelled enum
/// ([`crate::ops::types::FEDERATION_SOURCE_ERROR_CODE_VALUES`]) states.
pub const FEDERATION_SOURCE_OPERATION_NOT_SUPPORTED: &str = "OperationNotSupportedException";

/// The `FederationSourceErrorCode` of a source that IS served here and answered something
/// other than its objects: the value of the same modelled enum that states the source's
/// answer was not one the connector could read.
pub const FEDERATION_SOURCE_INVALID_RESPONSE: &str = "InvalidResponseException";

/// The data source a connection's connector reads — one variant per connection type whose
/// source this emulator serves itself ([`ENTITY_SOURCES`]).
///
/// None of them is reached through a SaaS API, which is what decides how
/// `ListEntitiesRequest$DataStoreApiVersion` is read: see
/// [`reached_through_a_saas_api`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntitySource {
    /// Amazon DynamoDB, whose entities are the TABLES the account holds in the Region the
    /// request is served in.
    DynamoDbTables,
}

/// One row per connection type whose data source this emulator serves, so that a connection
/// of that type has a connector here to read entities through. Every other modelled
/// `ConnectionType` reaches a source outside this emulator.
const ENTITY_SOURCES: &[(&str, EntitySource)] = &[("DYNAMODB", EntitySource::DynamoDbTables)];

/// The source a connection of `connection_type` is read through, or `None` when this
/// emulator serves no connector for that type.
pub fn entity_source(connection_type: &str) -> Option<EntitySource> {
    ENTITY_SOURCES
        .iter()
        .find_map(|(declared, source)| (*declared == connection_type).then_some(*source))
}

/// Whether `source` is reached through a SaaS API — the kind of source
/// `ListEntitiesRequest$DataStoreApiVersion` names a version of ("The API version of the
/// SaaS connector").
///
/// `false` for every source served here: Amazon DynamoDB is reached through its own data
/// plane, not through a SaaS application's API, so there is no SaaS connector version for
/// that member to select between and a read of such a source reports the same objects
/// whatever it carries. A connector that IS a SaaS one would publish the version it calls,
/// and the member would then select against it.
pub fn reached_through_a_saas_api(source: EntitySource) -> bool {
    match source {
        EntitySource::DynamoDbTables => false,
    }
}

/// Why the entities behind a connection could not be read. Both are federation failures —
/// the read is of another system — and they are told apart because they are about different
/// things: one is about the connection's SOURCE, the other about the answer it gave.
#[derive(Debug)]
pub enum EntitySourceRefusal {
    /// The connection's source is outside this emulator, so nothing here can enumerate what
    /// it holds.
    NoConnectorHere,
    /// The source is served here and answered something other than its objects. The detail
    /// is for this service's own log, never for the client.
    SourceNotReached(String),
}

/// ONE page of the entities a source holds.
#[derive(Debug, Default)]
pub struct EntityPage {
    /// The entities of this page, in the order the source listed them.
    pub entities: Vec<Entity>,
    /// Where the NEXT page resumes, as the source itself named that position, or `None` when
    /// this page is the last one.
    pub resume_from: Option<String>,
}

/// The page of entities that follows `after` — the position a previous page stopped at, or
/// the start of the listing for `None`.
pub async fn read_entity_page(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    source: EntitySource,
    after: Option<&str>,
) -> Result<EntityPage, EntitySourceRefusal> {
    match source {
        EntitySource::DynamoDbTables => read_dynamodb_tables(ctx, region, after).await,
    }
}

/// Whether the source holds an entity called `entity_name`, as the source's own read of that
/// one object answers.
pub async fn source_holds_entity(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    source: EntitySource,
    entity_name: &str,
) -> Result<bool, EntitySourceRefusal> {
    match source {
        EntitySource::DynamoDbTables => holds_dynamodb_table(ctx, region, entity_name).await,
    }
}

/// The page of Amazon DynamoDB TABLES that follows `after`.
///
/// One page of that store's own table listing is one page of this read: `ListTables` is
/// cursor-based over the table names it serves, so the position the next page resumes from is
/// the one it reports (`LastEvaluatedTableName`) and nothing of the store is buffered here.
async fn read_dynamodb_tables(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    after: Option<&str>,
) -> Result<EntityPage, EntitySourceRefusal> {
    let client = dynamodb_client(ctx, region);
    let listed = client
        .list_tables()
        .limit(ENTITIES_PER_PAGE)
        .set_exclusive_start_table_name(after.map(str::to_string))
        .send()
        .await
        .map_err(|error| {
            EntitySourceRefusal::SourceNotReached(format!(
                "the table store answered {} to the listing",
                DisplayErrorContext(&error)
            ))
        })?;
    Ok(EntityPage {
        entities: listed
            .table_names
            .unwrap_or_default()
            .into_iter()
            .map(table_entity)
            .collect(),
        resume_from: listed.last_evaluated_table_name,
    })
}

/// Whether Amazon DynamoDB holds the table `name`, decided by that store's own read of it:
/// the description it serves for a table it holds, and the miss it answers for one it does
/// not. Any other answer is a source this read could not reach.
async fn holds_dynamodb_table(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> Result<bool, EntitySourceRefusal> {
    let client = dynamodb_client(ctx, region);
    match client.describe_table().table_name(name).send().await {
        Ok(_) => Ok(true),
        Err(error) => {
            if error.as_service_error().is_some_and(
                ::aws_sdk_dynamodb::operation::describe_table::DescribeTableError::is_resource_not_found_exception,
            ) {
                return Ok(false);
            }
            Err(EntitySourceRefusal::SourceNotReached(format!(
                "the table store answered {} to the description",
                DisplayErrorContext(&error)
            )))
        }
    }
}

/// The entity ONE Amazon DynamoDB table is.
///
/// The table's own name is both the name the entity is addressed by and the `Label` it is
/// shown under — the store publishes no other name for a table. A table is one object with
/// nothing listed beneath it, so it is no parent entity, and its category is the one its
/// kind of object has ([`ENTITY_CATEGORY_TABLES`]). The store publishes neither a
/// description of a table nor any further property of one, so this connector returns none:
/// `Entity$CustomProperties` is "an optional map of keys which may be returned for an entity
/// by a connector".
fn table_entity(table: String) -> Entity {
    Entity {
        label: Some(table.clone()),
        entity_name: Some(table),
        is_parent_entity: Some(false),
        description: None,
        category: Some(ENTITY_CATEGORY_TABLES.to_string()),
        custom_properties: None,
    }
}
