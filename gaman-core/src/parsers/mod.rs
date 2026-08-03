//! Dialect-aware SQL parsers that lower DDL into Gaman schema state.
//!
//! The parser is a schema-loading frontend, not a migration parser. It accepts
//! only `CREATE` statements for entity kinds Gaman models in `Schema`: tables,
//! columns, constraints, foreign keys, indexes, extensions, triggers, functions,
//! views, and enums. `ALTER`, `DROP`, `DELETE`, and other lifecycle or DML
//! statements must be represented as migration operations, not parsed schema
//! input.
//!
//! SQL text is segmented before AST parsing. The segmenter is dialect-aware,
//! tag-free, and independent from lowering so unsupported SQL can still be
//! isolated into statement boundaries before the downstream parser rejects it.
//!
//! This module is the only boundary that uses the `sqlparser` crate. Public
//! parser APIs expose Gaman-owned types only: `Schema`, `Dialect`,
//! `SqlSegment`, `SqlStatementKind`, and `ParseError`.
//!
//! There is no default SQL dialect at this boundary. Callers must pass the
//! dialect explicitly for parsing and segmentation.

#![allow(clippy::result_large_err)]

mod common;
mod error;
mod mariadb;
mod mysql;
mod mysql_family;
mod postgres;
mod segments;
mod sql;
mod sqlite;
mod table_recovery;
pub(crate) mod tokens;

#[cfg(test)]
mod tests;

pub use error::ParseError;
pub use segments::{
    DdlStatementKind, DmlStatementKind, SqlObjectName, SqlSegment, SqlStatementKind, segment_sql,
};
pub use sql::parse_sql;
pub(crate) use sql::{OpaqueDeclaration, parse_opaque_create, parse_sql_raw};
