use crate::dialects::Dialect;
use crate::states::types::EntityKind;

/// Lexical classification for a segmented SQL statement.
///
/// This identifies broad statement intent from top-level tokens only. It does
/// not validate SQL syntax, dialect support, or Gaman lowering support.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlStatementKind {
    /// A modeled schema DDL target.
    Ddl(DdlStatementKind),
    /// A basic DML statement.
    Dml(DmlStatementKind),
}

/// A DDL statement target identified from `CREATE ... <entity> <name>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdlStatementKind {
    /// Gaman entity kind indicated by the statement determinant.
    pub entity: EntityKind,
    /// Best-effort object name following the determinant.
    pub name: Option<SqlObjectName>,
    /// Best-effort owner/target object for table-owned DDL such as indexes and triggers.
    pub owner: Option<SqlObjectName>,
}

/// A raw SQL object name captured without canonicalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlObjectName {
    /// Raw name as it appeared in the statement, including quote markers.
    pub raw: String,
    /// Dot-separated object name parts with identifier quote markers removed.
    pub parts: Vec<String>,
}

/// Basic DML statement kinds tracked by the segmenter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DmlStatementKind {
    /// A `SELECT` statement.
    Select,
    /// An `INSERT` statement.
    Insert,
    /// An `UPDATE` statement.
    Update,
    /// A `DELETE` statement.
    Delete,
    /// A CTE-backed DML statement.
    With(Box<DmlStatementKind>),
}

/// One raw SQL statement segment extracted from a larger SQL script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlSegment {
    /// One-based ordinal in the segmented input.
    pub ordinal: usize,
    /// High-confidence statement classification for Gaman-owned categories.
    pub kind: Option<SqlStatementKind>,
    /// Statement SQL without the statement terminator.
    ///
    /// This preserves leading whitespace and comments from the segment boundary
    /// so metadata comments attach to the following statement.
    pub sql: String,
    /// Half-open byte offset where this segment starts in the source SQL.
    ///
    /// This points to the beginning of the file or the byte immediately after
    /// the previous terminator/delimiter boundary. Leading comments are
    /// intentionally included.
    pub start_byte: usize,
    /// Half-open byte offset where this segment ends in the source SQL.
    ///
    /// This excludes the statement terminator or active delimiter and trims
    /// only trailing whitespace before that boundary.
    pub end_byte: usize,
    /// One-based start line for the returned SQL.
    pub start_line: usize,
    /// One-based start column for the returned SQL.
    pub start_column: usize,
    /// One-based end line for the returned SQL.
    pub end_line: usize,
    /// One-based end column for the returned SQL.
    pub end_column: usize,
}

pub(super) trait SqlSegmentationExt {
    fn supports_nested_block_comments(self) -> bool;
    fn supports_dollar_quotes(self) -> bool;
    fn supports_backtick_quotes(self) -> bool;
    fn supports_hash_comments(self) -> bool;
    fn supports_delimiter_directive(self) -> bool;
    fn tracks_sqlite_trigger_body(self) -> bool;
    fn tracks_mysql_body_blocks(self) -> bool;
    fn starts_statement(self, word: &str) -> bool;
}

impl SqlSegmentationExt for Dialect {
    fn supports_nested_block_comments(self) -> bool {
        matches!(self, Self::Postgres)
    }

    fn supports_dollar_quotes(self) -> bool {
        matches!(self, Self::Postgres)
    }

    fn supports_backtick_quotes(self) -> bool {
        matches!(self, Self::Mysql)
    }

    fn supports_hash_comments(self) -> bool {
        matches!(self, Self::Mysql)
    }

    fn supports_delimiter_directive(self) -> bool {
        matches!(self, Self::Mysql)
    }

    fn tracks_sqlite_trigger_body(self) -> bool {
        matches!(self, Self::Sqlite)
    }

    fn tracks_mysql_body_blocks(self) -> bool {
        matches!(self, Self::Mysql)
    }

    fn starts_statement(self, word: &str) -> bool {
        matches!(
            word,
            "CREATE"
                | "REPLACE"
                | "SELECT"
                | "WITH"
                | "INSERT"
                | "UPDATE"
                | "DELETE"
                | "ALTER"
                | "DROP"
                | "TRUNCATE"
                | "MERGE"
                | "CALL"
                | "EXPLAIN"
                | "BEGIN"
                | "COMMIT"
                | "ROLLBACK"
        ) || matches!(
            (self, word),
            (Self::Sqlite, "PRAGMA" | "VACUUM" | "ANALYZE" | "REINDEX")
                | (
                    Self::Mysql,
                    "USE" | "SHOW" | "DESCRIBE" | "DESC" | "LOCK" | "UNLOCK" | "SET"
                )
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Location {
    pub(super) line: usize,
    pub(super) column: usize,
}

impl Location {
    pub(super) fn new(line: usize, column: usize) -> Self {
        Self { line, column }
    }
}
