// SPDX-License-Identifier: CC0-1.0

//! The SCHEMA a statement's names are resolved in, and the catalog a statement that reads
//! the database's own schema reads.
//!
//! PostgreSQL resolves an unqualified name in the schema search path, whose first entry is
//! `public` — "the default schema, in which user objects are created" — and answers "what
//! tables does this database hold?" out of the catalog schemas `information_schema` and
//! `pg_catalog`. The engine behind this data plane has ONE schema of stored objects and no
//! schema namespace of its own, so a statement that spells a name the way PostgreSQL does
//! has to be read the same way:
//!
//! * `public.<relation>` is the relation itself — the schema the engine's objects are in,
//!   so the qualification selects exactly what the bare name does, in every position a
//!   statement names a relation in (what a query reads from, what a definition, a delete or
//!   an assignment addresses, and a column reference qualified by both);
//! * a relation of a CATALOG SCHEMA — `information_schema` or `pg_catalog`, which every
//!   PostgreSQL database is created with — is a VIEW OF THE SCHEMA, so each one this module
//!   serves is rewritten into the query over the engine's own schema that answers it. Every
//!   value such a row carries is read out of that schema — the relation's name, its kind,
//!   its columns and the types they were declared with, whether it carries an index or a
//!   trigger — so nothing about the database is invented, and a catalog relation this module
//!   does not serve is left as the caller wrote it, which the engine answers as the relation
//!   it has none of.
//!
//! `pg_catalog` is reached by a BARE name as well as by a qualified one: "pg_catalog is
//! effectively part of the search path … if it is not named explicitly in the path then it
//! is implicitly searched before searching the path's schemas", so a bare `pg_tables` names
//! that catalog's view wherever it appears, which is how client tooling spells a
//! catalog read. `information_schema` is not on that implicit path, so only a name
//! qualified by it reaches its relations.
//!
//! Each served relation also carries the TYPE the catalog declares each of its columns
//! with ([`Relation::columns`]), which is what a statement selecting one of those columns
//! reports for it: the rewritten relation is a derived table, so the engine has no
//! declaration of its own to report and a column PostgreSQL declares as a boolean would
//! otherwise come back as the integer the engine holds it as.
//!
//! A rewritten relation becomes a derived table of the caller's OWN statement rather than a
//! view stored in the database: nothing is added to the schema the caller reads back
//! (`information_schema.tables` would otherwise report the catalog itself), and the schema
//! introspection the rewrite uses is run by the statement, which is the one place this data
//! plane's authorizer admits it (`crate::state::confine_to_data_plane`).

use std::ops::ControlFlow;

use sqlparser::ast::{
    Expr, Ident, ObjectName, ObjectNamePart, Query, TableAlias, TableFactor, TableObject, VisitMut,
    VisitorMut,
};

/// The schema PostgreSQL creates every database with and resolves an unqualified name in
/// first, and so the schema the engine's own objects are reported in.
pub(crate) const PUBLIC_SCHEMA: &str = "public";

/// The schema whose relations describe the database itself — "a schema named
/// `information_schema` ... containing views that describe the objects of the current
/// database".
const INFORMATION_SCHEMA: &str = "information_schema";

/// PostgreSQL's OWN catalog schema, which "contains the system tables and all the built-in
/// data types, functions, and operators" and is implicitly searched before the schemas of
/// the search path, so a name of one of its relations resolves to it whether or not the
/// statement qualifies it.
const PG_CATALOG: &str = "pg_catalog";

/// The database PostgreSQL creates in every cluster, which is the catalog name a statement
/// run against this data plane's DEFAULT database reports: a request that names no
/// `database` addresses the cluster's own default one, and PostgreSQL's is `postgres`.
const DEFAULT_DATABASE: &str = "postgres";

/// The name the database a statement runs against is REPORTED under: the name the request's
/// own `database` member named, and the database PostgreSQL creates in every cluster for a
/// request that named none.
///
/// The ONE reading of it, so the catalog's `table_catalog` and the server's own
/// `current_database()` ([`crate::state::dialect_builtins`]) report one name.
pub(crate) fn catalog_name(database: &str) -> &str {
    if database.is_empty() {
        DEFAULT_DATABASE
    } else {
        database
    }
}

/// The engine's own schema table, which every catalog relation below is read out of: it
/// carries one row per table, view, index and trigger the database holds.
const SCHEMA_TABLE: &str = "sqlite_master";

/// The engine's own objects, which PostgreSQL's catalog has no row for: the schema table
/// itself and the sequence bookkeeping the engine keeps under the same reserved prefix.
const ENGINE_OBJECT_PREFIX: &str = "sqlite_";

/// Resolve the schema every name in `statement` is qualified by, against the one schema of
/// stored objects this data plane has and the catalog relations it serves for `database`,
/// whose objects `user` owns ([`Relation`]).
///
/// Answers the catalog relations the statement READS, so the caller can report the type the
/// catalog declares each of their columns with ([`catalog_column_types`]).
///
/// DEPTH: the walk descends the statement tree, which [`super::plan`] has already bounded
/// before a tree exists at all — the parser's own recursion limit and the token pass over
/// the levels an operator chain asks for — exactly as the delimiting walk beside it is
/// bounded ([`super::names`]).
pub(super) fn resolve_schemas(
    statement: &mut sqlparser::ast::Statement,
    database: &str,
    user: &str,
) -> Vec<&'static Relation> {
    let named = catalog_name(database);
    // The database's name and the session's user are CALLER-SUPPLIED text and the relations
    // below report them as values, so each is written as ONE string literal whatever it
    // holds: the standard doubling of a quote, so a name carrying one states that character
    // rather than ending the literal and spelling something else in the query this builds.
    let mut walk = Schemas {
        catalog_name: quoted_literal(named),
        // The owner of every object in this database is the role that created it, and a
        // statement reaching this data plane is run by the user its call authenticated as.
        // A database no call has resolved a user for has no role to report, which is a
        // value the catalog carries no row of its own for either.
        owner: (!user.is_empty()).then(|| quoted_literal(user)),
        served: Vec::new(),
    };
    // The visitor cannot break, so the walk always completes.
    let _ = statement.visit(&mut walk);
    walk.served
}

/// `text` as the body of a SQL string literal: the standard doubling of a quote.
fn quoted_literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// The walk that resolves the qualifications. It visits the WHOLE statement, so a catalog
/// relation read inside a subquery or a common table expression is resolved like one in the
/// statement's own clauses.
struct Schemas {
    /// The name the catalog reports as the database a relation belongs to, already written
    /// as a string literal.
    catalog_name: String,
    /// The role the catalog reports as the owner of the database's objects, already written
    /// as a string literal; `None` where no call has resolved one.
    owner: Option<String>,
    /// The catalog relations this walk rewrote, in the order it met them.
    served: Vec<&'static Relation>,
}

impl VisitorMut for Schemas {
    type Break = ();

    /// A relation the statement READS FROM, which is the one position a catalog relation
    /// can be read in: a served one becomes the query that answers it.
    ///
    /// A `public.` qualification is not resolved here but in [`Self::pre_visit_relation`]
    /// and [`Self::pre_visit_statement`], which reach every position a statement names a
    /// relation in rather than this one alone.
    fn pre_visit_table_factor(&mut self, factor: &mut TableFactor) -> ControlFlow<Self::Break> {
        let TableFactor::Table { name, alias, .. } = factor else {
            return ControlFlow::Continue(());
        };
        // A catalog relation this module does not serve is left exactly as the caller wrote
        // it, so the engine answers that the database holds no such relation.
        let Some(relation) = catalog_relation(name) else {
            return ControlFlow::Continue(());
        };
        let Some(query) = self.catalog_query(relation) else {
            return ControlFlow::Continue(());
        };
        // A derived table is addressed by the alias the caller gave it, else by the
        // relation's own name — which is what a reference to `tables.table_name` names.
        let named = alias.take().unwrap_or_else(|| TableAlias {
            name: Ident::new(relation.name),
            columns: Vec::new(),
        });
        *factor = TableFactor::Derived {
            lateral: false,
            subquery: query,
            alias: Some(named),
        };
        self.served.push(relation);
        ControlFlow::Continue(())
    }

    /// A relation the statement NAMES, wherever it names one: what a query reads from, and
    /// what a definition, a delete or an assignment addresses. The schema the engine's own
    /// objects are in selects exactly what the bare name does, so the qualification is
    /// resolved away in every one of those positions.
    fn pre_visit_relation(&mut self, relation: &mut ObjectName) -> ControlFlow<Self::Break> {
        resolve_public(relation);
        ControlFlow::Continue(())
    }

    /// The relation an INSERT addresses, which the statement carries as a table OBJECT
    /// rather than as a relation of its own, so the walk above does not reach it.
    fn pre_visit_statement(
        &mut self,
        statement: &mut sqlparser::ast::Statement,
    ) -> ControlFlow<Self::Break> {
        if let sqlparser::ast::Statement::Insert(insert) = statement {
            if let TableObject::TableName(name) = &mut insert.table {
                resolve_public(name);
            }
        }
        ControlFlow::Continue(())
    }

    /// A column reference qualified by the SCHEMA as well as by the relation
    /// (`public.t.c`, `information_schema.tables.table_name`): the schema is resolved
    /// away, leaving the reference the relation itself answers.
    fn pre_visit_expr(&mut self, expr: &mut Expr) -> ControlFlow<Self::Break> {
        if let Expr::CompoundIdentifier(parts) = expr {
            if parts.len() == 3 && is_resolved_schema(&parts[0]) {
                parts.remove(0);
            }
        }
        ControlFlow::Continue(())
    }
}

/// Take the schema the engine's own objects are in off a relation qualified by it, leaving
/// the name the engine resolves. A name qualified by any other schema is left as the caller
/// wrote it.
fn resolve_public(name: &mut ObjectName) {
    if let Some((schema, relation)) = qualified(name) {
        if schema == PUBLIC_SCHEMA {
            *name = ObjectName(vec![ObjectNamePart::Identifier(Ident::new(relation))]);
        }
    }
}

impl Schemas {
    /// The query over the engine's own schema that answers the catalog relation
    /// `relation`, or `None` where the query this module builds for it does not parse.
    fn catalog_query(&self, relation: &'static Relation) -> Option<Box<Query>> {
        let sql = match (relation.schema, relation.name) {
            (INFORMATION_SCHEMA, "tables") => self.tables_query(),
            (INFORMATION_SCHEMA, "columns") => self.columns_query(),
            (PG_CATALOG, "pg_tables") => self.pg_tables_query(),
            // Every entry of [`SERVED`] is answered above; a relation that reached here
            // without one would be a defect of this module rather than of the statement.
            _ => return None,
        };
        // The text is this module's own, and it is parsed by the same reader every
        // statement is: a rewrite that did not parse would be a defect here, so it is
        // reported as one rather than changing the statement into something else.
        match super::parse_query(&sql) {
            Some(query) => Some(query),
            None => {
                ::tracing::error!(%sql, "the catalog query this emulator builds does not parse");
                None
            }
        }
    }

    /// `information_schema.tables` — "one row for each table or view in the current
    /// database", with `table_type` `BASE TABLE` for a table and `VIEW` for a view.
    ///
    /// Every row is a row of the engine's own schema table, so a relation the database does
    /// not hold is in no answer.
    fn tables_query(&self) -> String {
        format!(
            "SELECT {catalog} AS table_catalog, '{PUBLIC_SCHEMA}' AS table_schema, \
             name AS table_name, \
             CASE type WHEN 'view' THEN 'VIEW' ELSE 'BASE TABLE' END AS table_type \
             FROM {SCHEMA_TABLE} \
             WHERE type IN ('table', 'view') AND name NOT LIKE '{ENGINE_OBJECT_PREFIX}%'",
            catalog = self.catalog_name
        )
    }

    /// `information_schema.columns` — "one row for each column in a table or view of the
    /// current database", in the position each relation declares it at, with the type it
    /// was declared with and whether it admits a null.
    ///
    /// The columns of a relation are read with the schema introspection the engine itself
    /// answers for that relation (`pragma_table_info`), so a column no relation declares is
    /// in no answer either.
    fn columns_query(&self) -> String {
        format!(
            "SELECT {catalog} AS table_catalog, '{PUBLIC_SCHEMA}' AS table_schema, \
             relation.name AS table_name, declared.name AS column_name, \
             declared.cid + 1 AS ordinal_position, \
             CASE declared.\"notnull\" WHEN 0 THEN 'YES' ELSE 'NO' END AS is_nullable, \
             lower(declared.type) AS data_type, declared.dflt_value AS column_default \
             FROM {SCHEMA_TABLE} AS relation \
             JOIN pragma_table_info(relation.name) AS declared \
             WHERE relation.type IN ('table', 'view') \
             AND relation.name NOT LIKE '{ENGINE_OBJECT_PREFIX}%'",
            catalog = self.catalog_name
        )
    }

    /// `pg_catalog.pg_tables` — PostgreSQL's view that "provides access to useful
    /// information about each table in the database", one row per TABLE (its views are
    /// `pg_views`), with the schema it is in, its owner, the tablespace it is stored in and
    /// whether it carries indexes, rules, triggers or row security.
    ///
    /// Every value is read out of the database's own schema: the table's name, whether the
    /// engine holds an index on it or its declaration carries a key — PostgreSQL's
    /// `relhasindex` is "true if this is a table and it has (or recently had) any indexes",
    /// which a primary key is one of — and whether a trigger is defined on it. The
    /// tablespace is the one a relation "in the database's default tablespace" reports, which
    /// is none, and a RULE and row-level security are definitions the statements this data
    /// plane runs declare none of, so no table carries either.
    ///
    /// The table's name is read as the value of this view's own column rather than as the
    /// engine's column it comes out of, so the column is reported with the type the CATALOG
    /// declares it with ([`SERVED`]) and not with the engine's declaration for the schema
    /// table it was read from.
    fn pg_tables_query(&self) -> String {
        format!(
            "SELECT '{PUBLIC_SCHEMA}' AS schemaname, \
             CAST(relation.name AS TEXT) AS tablename, \
             {owner} AS tableowner, NULL AS tablespace, \
             (EXISTS (SELECT 1 FROM {SCHEMA_TABLE} AS held \
               WHERE held.type = 'index' AND held.tbl_name = relation.name) \
              OR EXISTS (SELECT 1 FROM pragma_table_info(relation.name) AS declared \
               WHERE declared.pk > 0)) AS hasindexes, \
             FALSE AS hasrules, \
             EXISTS (SELECT 1 FROM {SCHEMA_TABLE} AS fired \
               WHERE fired.type = 'trigger' AND fired.tbl_name = relation.name) \
              AS hastriggers, \
             FALSE AS rowsecurity \
             FROM {SCHEMA_TABLE} AS relation \
             WHERE relation.type = 'table' \
             AND relation.name NOT LIKE '{ENGINE_OBJECT_PREFIX}%'",
            owner = self.owner.as_deref().unwrap_or("NULL")
        )
    }
}

/// One relation of a catalog schema this module serves.
pub(super) struct Relation {
    /// The catalog schema it belongs to.
    schema: &'static str,
    /// Its own name, as PostgreSQL holds it: lower case.
    pub(super) name: &'static str,
    /// Each column it carries, under the catalog's own name for it, with the type that
    /// catalog declares it with — which is the type a statement selecting the column
    /// reports, except where the engine has a declaration of its own for the value the
    /// column is read out of ([`Schemas::pg_tables_query`]).
    columns: &'static [(&'static str, &'static str)],
}

impl Relation {
    /// The type this relation declares the column `column` with, and `None` for a name it
    /// carries no column of.
    fn column_type(&self, column: &str) -> Option<&'static str> {
        self.columns
            .iter()
            .find(|(named, _)| named.eq_ignore_ascii_case(column))
            .map(|(_, declared)| *declared)
    }
}

/// The catalog relations this module serves, and the type each of their columns is
/// declared with.
///
/// `information_schema`'s own column types are the domains that catalog declares — a
/// `sql_identifier` is a `name`, a `cardinal_number` a non-negative integer and
/// `character_data` a character string — and `pg_catalog.pg_tables`' are the view's own:
/// the four names of its first columns and the four booleans after them.
const SERVED: &[Relation] = &[
    Relation {
        schema: INFORMATION_SCHEMA,
        name: "tables",
        columns: &[
            ("table_catalog", "name"),
            ("table_schema", "name"),
            ("table_name", "name"),
            ("table_type", "varchar"),
        ],
    },
    Relation {
        schema: INFORMATION_SCHEMA,
        name: "columns",
        columns: &[
            ("table_catalog", "name"),
            ("table_schema", "name"),
            ("table_name", "name"),
            ("column_name", "name"),
            ("ordinal_position", "int4"),
            ("is_nullable", "varchar"),
            ("data_type", "varchar"),
            ("column_default", "varchar"),
        ],
    },
    Relation {
        schema: PG_CATALOG,
        name: "pg_tables",
        columns: &[
            ("schemaname", "name"),
            ("tablename", "name"),
            ("tableowner", "name"),
            ("tablespace", "name"),
            ("hasindexes", "bool"),
            ("hasrules", "bool"),
            ("hastriggers", "bool"),
            ("rowsecurity", "bool"),
        ],
    },
];

/// The catalog relation a statement's relation name resolves to, and `None` for every name
/// that is not one this module serves.
///
/// A two-part name names the relation of the schema it qualifies. A BARE name names a
/// relation of `pg_catalog`, which PostgreSQL searches implicitly ahead of the search
/// path's own schemas, and nothing of `information_schema`, which is reached only through
/// its own qualification.
fn catalog_relation(name: &ObjectName) -> Option<&'static Relation> {
    match (qualified(name), bare(name)) {
        (Some((schema, relation)), _) => served(&schema, &relation),
        (_, Some(relation)) => served(PG_CATALOG, &relation),
        _ => None,
    }
}

/// The served relation `relation` of the schema `schema`.
fn served(schema: &str, relation: &str) -> Option<&'static Relation> {
    SERVED
        .iter()
        .find(|served| served.schema == schema && served.name == relation)
}

/// Report the type the catalog declares for each result column of `statement` that READS a
/// column of one of the catalog relations `served` for it.
///
/// What this is for: a served relation is rewritten into a derived table, so the engine
/// reports no declared type for a column read out of it and the value's own storage class
/// would decide how it comes back — which would return a column PostgreSQL declares as a
/// boolean as the integer the engine holds it in. The type belongs to the CATALOG's
/// declaration of the column, so it is reported from there, for a result column that is a
/// reference to one of those columns (`tablename`, `pg_tables.tablename`) and never for a
/// value the statement computed and happened to label with the same name.
///
/// A statement selecting EVERY column of one served relation names none of them, so the
/// relation's own columns are what its result set carries, in the order the relation
/// declares them: the order this module's query projects them in. A wildcard over anything
/// further than that one relation is left alone — the types then no longer cover the result
/// set, which is exactly when nothing may be read off a position ([`super::Plan::result_type`]).
pub(super) fn catalog_column_types(
    statement: &sqlparser::ast::Statement,
    served: &[&'static Relation],
    types: &mut Vec<Option<super::PgType>>,
) {
    if served.is_empty() {
        return;
    }
    let sqlparser::ast::Statement::Query(query) = statement else {
        return;
    };
    let Some(projection) = super::leftmost_projection(&query.body) else {
        return;
    };
    if let ([sqlparser::ast::SelectItem::Wildcard(_)], [relation]) = (projection, served) {
        *types = relation
            .columns
            .iter()
            .map(|(_, declared)| Some(super::pg_type(declared)))
            .collect();
        return;
    }
    if projection.len() != types.len() {
        return;
    }
    for (item, reported) in projection.iter().zip(types.iter_mut()) {
        let Some(column) = selected_column(item) else {
            continue;
        };
        if let Some(declared) = served
            .iter()
            .find_map(|relation| relation.column_type(&column))
        {
            *reported = Some(super::pg_type(declared));
        }
    }
}

/// The column one selected item is a REFERENCE to — written bare or qualified by the
/// relation it is read from, which is the column's own name in either spelling — and `None`
/// for an item that is not a reference to a column at all.
fn selected_column(item: &sqlparser::ast::SelectItem) -> Option<String> {
    use sqlparser::ast::SelectItem;
    let expr = match item {
        SelectItem::UnnamedExpr(expr) | SelectItem::ExprWithAlias { expr, .. } => expr,
        _ => return None,
    };
    match expr {
        Expr::Identifier(name) => Some(name.value.to_ascii_lowercase()),
        Expr::CompoundIdentifier(parts) => parts
            .last()
            .map(|column| column.value.to_ascii_lowercase()),
        _ => None,
    }
}

/// Whether an identifier names a schema this module resolves away — the schema the engine's
/// objects are in, or a catalog schema whose relations are rewritten.
fn is_resolved_schema(part: &Ident) -> bool {
    let name = part.value.to_ascii_lowercase();
    name == PUBLIC_SCHEMA || name == INFORMATION_SCHEMA || name == PG_CATALOG
}

/// The `(schema, relation)` a two-part name spells, folded to lower case the way
/// PostgreSQL folds an undelimited name; `None` for a name of any other shape.
///
/// A name the caller DELIMITED is folded here too: the engine compares identifiers without
/// regard to case whichever way they are written ([`super::names`]), so the two spellings
/// select the same relation in a statement this data plane runs, and the catalog's own
/// relation names are lower case in PostgreSQL.
fn qualified(name: &ObjectName) -> Option<(String, String)> {
    let [first, second] = &name.0[..] else {
        return None;
    };
    let (ObjectNamePart::Identifier(schema), ObjectNamePart::Identifier(relation)) =
        (first, second)
    else {
        return None;
    };
    Some((
        schema.value.to_ascii_lowercase(),
        relation.value.to_ascii_lowercase(),
    ))
}

/// The relation a ONE-part name spells, folded the way [`qualified`] folds a qualified one;
/// `None` for a name of any other shape.
fn bare(name: &ObjectName) -> Option<String> {
    let [only] = &name.0[..] else {
        return None;
    };
    let ObjectNamePart::Identifier(relation) = only else {
        return None;
    };
    Some(relation.value.to_ascii_lowercase())
}

