use crate::dialects::Dialect;

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("{dialect} SQL parse error: {message}")]
    Parse {
        dialect: &'static str,
        message: String,
    },
    #[error("unsupported {dialect} SQL statement '{statement}': {reason}")]
    UnsupportedStatement {
        dialect: &'static str,
        statement: String,
        reason: String,
    },
    #[error("{dialect} SQL segmentation error at line {line}, column {column}: {reason}")]
    Segment {
        dialect: &'static str,
        line: usize,
        column: usize,
        reason: String,
    },
    #[error("unsupported SQL dialect '{0}'")]
    UnsupportedDialect(String),
    #[error("CREATE INDEX references unknown table '{table}'")]
    UnknownTable { table: String },
    #[error("CREATE TRIGGER references unknown table '{table}'")]
    UnknownTriggerTable { table: String },
    #[error("duplicate table '{0}'")]
    DuplicateTable(String),
    #[error("cannot read '{path}': {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

impl ParseError {
    pub(crate) fn parse(dialect: Dialect, message: impl Into<String>) -> Self {
        Self::Parse {
            dialect: dialect.as_str(),
            message: message.into(),
        }
    }

    pub(crate) fn unsupported(
        dialect: Dialect,
        statement: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self::UnsupportedStatement {
            dialect: dialect.as_str(),
            statement: statement.into(),
            reason: reason.into(),
        }
    }

    pub(crate) fn segment(
        dialect: Dialect,
        line: usize,
        column: usize,
        reason: impl Into<String>,
    ) -> Self {
        Self::Segment {
            dialect: dialect.as_str(),
            line,
            column,
            reason: reason.into(),
        }
    }
}
