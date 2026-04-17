#[derive(Debug, thiserror::Error)]
pub enum SqlParseError {
    #[error("SQL parse error: {0}")]
    Parse(String),
    #[error("unsupported SQL statement: {stmt}")]
    Unsupported { stmt: String },
    #[error("CREATE INDEX references unknown table '{table}'")]
    UnknownTable { table: String },
    #[error("duplicate table '{0}'")]
    DuplicateTable(String),
    #[error("cannot read '{path}': {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}
