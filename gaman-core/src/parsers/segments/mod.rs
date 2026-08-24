//! SQL statement segmentation for parser frontends.
//!
//! This module splits SQL scripts into `SqlSegment` values before AST parsing.
//! Segmentation is dialect-aware and tag-free so unsupported statements can
//! still be isolated and reported accurately by later parser stages.
//!
//! Segments preserve leading whitespace and comments from the segment boundary.
//! This lets metadata comments attach to the following statement. Segment byte
//! offsets are half-open source ranges, and `segment.sql` is always the exact
//! source slice at `start_byte..end_byte`, excluding terminators and active
//! delimiters.
//!
//! Segment classification is lexical metadata. It recognizes a statement's
//! broad kind and primary object name from top-level tokens only; it does not
//! validate SQL syntax, dialect support, or Gaman lowering support.

pub mod annotations;
mod classifier;
mod scanner;
mod types;

#[cfg(test)]
mod tests;

pub use annotations::SqlAnnotation;
pub use scanner::segment_sql;
pub use types::{DdlStatementKind, DmlStatementKind, SqlObjectName, SqlSegment, SqlStatementKind};
