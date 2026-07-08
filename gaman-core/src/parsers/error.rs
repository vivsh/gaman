use crate::dialects::Dialect;
use crate::parsers::segments::SqlSegment;
use sqlparser::parser::ParserError;

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("{dialect} SQL parse error{location}: {message}")]
    Parse {
        dialect: &'static str,
        message: String,
        line: Option<usize>,
        column: Option<usize>,
        segment_ordinal: Option<usize>,
        segment_start_line: Option<usize>,
        segment_start_column: Option<usize>,
        location: String,
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
    pub(crate) fn parse_in_segment(
        dialect: Dialect,
        segment: &SqlSegment,
        error: ParserError,
    ) -> Self {
        let message = error.to_string();
        let (line, column) = extract_sqlparser_location(&message)
            .map(|(line, column)| remap_segment_location(segment, line, column))
            .unwrap_or((None, None));
        let location = parse_location_suffix(line, column, segment);

        Self::Parse {
            dialect: dialect.as_str(),
            message,
            line,
            column,
            segment_ordinal: Some(segment.ordinal),
            segment_start_line: Some(segment.start_line),
            segment_start_column: Some(segment.start_column),
            location,
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

fn extract_sqlparser_location(message: &str) -> Option<(usize, usize)> {
    let (_, location) = message.rsplit_once(" at Line: ")?;
    let (line, column) = location.split_once(", Column: ")?;
    Some((line.parse().ok()?, column.parse().ok()?))
}

fn remap_segment_location(
    segment: &SqlSegment,
    line: usize,
    column: usize,
) -> (Option<usize>, Option<usize>) {
    let source_line = segment.start_line + line.saturating_sub(1);
    let source_column = if line == 1 {
        segment.start_column + column.saturating_sub(1)
    } else {
        column
    };
    (Some(source_line), Some(source_column))
}

fn parse_location_suffix(
    line: Option<usize>,
    column: Option<usize>,
    segment: &SqlSegment,
) -> String {
    match (line, column) {
        (Some(line), Some(column)) => format!(" at line {line}, column {column}"),
        _ => format!(
            " near segment {} starting at line {}, column {}",
            segment.ordinal, segment.start_line, segment.start_column
        ),
    }
}
