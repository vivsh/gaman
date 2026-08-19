//! Shared root-entity selector syntax for filters and explicit dependencies.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::states::EntityKind;

/// Parsed canonical `kind::target` selector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntitySelector {
    /// Root entity kind.
    pub kind: EntityKind,
    /// Qualified identity, signature, or filter pattern.
    pub target: String,
}

/// Invocation-scoped root entity filter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityFilter {
    /// Root entity kind selected by this filter.
    pub kind: EntityKind,
    /// Glob matched against canonical qualified identities.
    pub pattern: String,
}

/// Explicit exact root dependency resolved during schema preparation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityDependency {
    /// Referenced root kind.
    pub kind: EntityKind,
    /// Exact qualified identity or function signature.
    pub target: String,
}

impl Serialize for EntityDependency {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{}::{}", kind_name(self.kind), self.target))
    }
}

impl<'de> Deserialize<'de> for EntityDependency {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let source = String::deserialize(deserializer)?;
        Self::parse(&source).map_err(serde::de::Error::custom)
    }
}

impl EntitySelector {
    /// Parses a filter, accepting canonical double-colon and legacy colon syntax.
    pub fn parse_filter(value: &str) -> Result<EntityFilter, String> {
        let (kind, target) = if let Some((kind, target)) = value.split_once("::") {
            (parse_kind(kind)?, target)
        } else if let Some((kind, target)) = value.split_once(':') {
            (parse_kind(kind)?, target)
        } else {
            (EntityKind::Table, value)
        };
        if target.trim().is_empty() {
            return Err("entity filter pattern is empty".to_string());
        }
        Ok(EntityFilter { kind, pattern: target.to_string() })
    }

    /// Parses one exact dependency without permitting globs or legacy syntax.
    pub fn parse_dependency(value: &str) -> Result<EntityDependency, String> {
        let (kind, target) = value
            .split_once("::")
            .ok_or_else(|| "dependencies must use kind::target syntax".to_string())?;
        let kind = parse_kind(kind)?;
        let target = target.trim();
        if target.is_empty() || target.contains('*') || target.contains('?') {
            return Err("dependency target must be a non-glob identity".to_string());
        }
        if kind == EntityKind::Function && !valid_function_target(target) {
            return Err("function dependency signature is malformed".to_string());
        }
        Ok(EntityDependency { kind, target: target.to_string() })
    }
}

fn valid_function_target(target: &str) -> bool {
    let Some(open) = target.find('(') else {
        return !target.contains(')') && !target.trim().is_empty();
    };
    if open == 0 || !target.ends_with(')') {
        return false;
    }
    let mut depth = 0usize;
    for character in target[open..].chars() {
        match character {
            '(' => depth += 1,
            ')' if depth == 0 => return false,
            ')' => depth -= 1,
            _ => {}
        }
    }
    depth == 0
}

impl EntityDependency {
    /// Parses one dependency with the shared exact selector grammar.
    pub fn parse(value: &str) -> Result<Self, String> {
        EntitySelector::parse_dependency(value)
    }
}

fn parse_kind(value: &str) -> Result<EntityKind, String> {
    match value.trim() {
        "table" => Ok(EntityKind::Table),
        "function" => Ok(EntityKind::Function),
        "view" => Ok(EntityKind::View),
        "enum" => Ok(EntityKind::Enum),
        "extension" => Ok(EntityKind::Extension),
        _ => Err(format!("unknown entity selector kind '{value}'")),
    }
}

fn kind_name(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Table => "table",
        EntityKind::Function => "function",
        EntityKind::View => "view",
        EntityKind::Enum => "enum",
        EntityKind::Extension => "extension",
        _ => "entity",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies filters accept canonical double-colon syntax and the legacy alias.
    #[test]
    fn filters_accept_canonical_and_legacy_syntax() {
        assert_eq!(EntitySelector::parse_filter("table::users*").unwrap().kind, EntityKind::Table);
        assert_eq!(EntitySelector::parse_filter("function:daily*").unwrap().kind, EntityKind::Function);
    }

    /// Verifies dependencies reject glob patterns and preserve canonical typed signatures.
    #[test]
    fn dependencies_are_exact_and_serialize_canonically() {
        let dependency = EntityDependency::parse("function::daily(date, integer)").unwrap();
        assert_eq!(serde_yaml::to_string(&dependency).unwrap().trim(), "function::daily(date, integer)");
        assert!(EntityDependency::parse("function::daily*").is_err());
        assert!(EntityDependency::parse("function::daily(date))").is_err());
    }
}
