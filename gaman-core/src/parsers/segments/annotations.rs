//! Strict leading SQL annotations attached to one segmented statement.

use crate::entity_selector::EntitySelector;

/// One closed, source-ordered SQL annotation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlAnnotation {
    /// Explicit entity dependency for the following function declaration.
    DependsOn {
        /// Exact root dependency carried by this directive.
        dependency: crate::EntityDependency,
        /// Source span of the complete directive comment.
        span: SqlAnnotationSpan,
    },
}

/// Byte and line location of one source annotation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlAnnotationSpan {
    /// Zero-based byte offset within the segmented statement.
    pub start_byte: usize,
    /// Exclusive zero-based byte offset within the segmented statement.
    pub end_byte: usize,
    /// One-based line within the segmented statement.
    pub line: usize,
    /// One-based column within the segmented statement.
    pub column: usize,
}

/// Parses reserved leading `-- @directive payload` comments.
pub(super) fn parse(source: &str) -> Result<(Vec<SqlAnnotation>, String), String> {
    let mut annotations = Vec::new();
    let mut stripped = String::with_capacity(source.len());
    let mut leading = true;
    let mut start_byte = 0usize;
    for (line_index, line) in source.split_inclusive('\n').enumerate() {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let newline = &line[content.len()..];
        let trimmed = content.trim_start();
        if trimmed.is_empty() {
            stripped.push_str(line);
            start_byte += line.len();
            continue;
        }
        if leading && trimmed.starts_with("--") {
            let comment = trimmed.trim_start_matches("--").trim_start();
            if let Some(rest) = comment.strip_prefix('@') {
                let (directive, payload) = rest
                    .split_once(char::is_whitespace)
                    .ok_or_else(|| "SQL annotation is missing its payload".to_string())?;
                match directive {
                    "depends-on" => annotations.push(SqlAnnotation::DependsOn {
                        dependency: EntitySelector::parse_dependency(payload)?,
                        span: SqlAnnotationSpan {
                            start_byte,
                            end_byte: start_byte + content.len(),
                            line: line_index + 1,
                            column: 1,
                        },
                    }),
                    _ => return Err(format!("unknown SQL annotation '@{directive}'")),
                }
                stripped.extend(
                    content
                        .chars()
                        .map(|character| if character == '\r' { '\r' } else { ' ' }),
                );
                stripped.push_str(newline);
                start_byte += line.len();
                continue;
            }
            stripped.push_str(line);
            start_byte += line.len();
            continue;
        }
        leading = false;
        stripped.push_str(line);
        start_byte += line.len();
    }
    Ok((annotations, stripped))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies leading dependency annotations parse without treating body comments as metadata.
    #[test]
    fn parses_only_leading_dependency_annotations() {
        let (annotations, _) = parse("-- @depends-on function::daily(date)\nCREATE FUNCTION daily_report() RETURNS int LANGUAGE sql AS $$ -- @unknown x\nSELECT 1 $$")
            .expect("annotation parsing");
        assert!(
            matches!(annotations.as_slice(), [SqlAnnotation::DependsOn { dependency, span }] if dependency.target == "daily(date)" && span.line == 1)
        );
    }

    /// Verifies unknown reserved leading annotations fail instead of being silently ignored.
    #[test]
    fn rejects_unknown_leading_annotation() {
        assert!(parse("-- @unknown value\nCREATE FUNCTION daily() RETURNS int LANGUAGE sql AS $$ SELECT 1 $$").is_err());
    }

    /// Verifies stripped directive text retains source line and byte alignment for diagnostics.
    #[test]
    fn strips_only_reserved_leading_directives() {
        let source = "-- @depends-on function::daily()\nCREATE FUNCTION report() RETURNS int LANGUAGE sql AS $$\n-- @depends-on ignored\nSELECT 1 $$";
        let (_, stripped) = parse(source).expect("annotation parsing");
        assert_eq!(source.len(), stripped.len());
        assert!(
            stripped
                .lines()
                .next()
                .expect("directive line")
                .trim()
                .is_empty()
        );
        assert!(stripped.contains("-- @depends-on ignored"));
    }
}
