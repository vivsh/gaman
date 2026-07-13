//! User-facing documentation derived from registered semantic drift properties.

use crate::states::EntityKind;

/// Documentation for one dialect property comparator used by `verify`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftPropertyDoc {
    /// Entity kind whose modeled form owns this property.
    pub entity_kind: EntityKind,
    /// Stable property identifier used in drift findings.
    pub property: &'static str,
    /// Semantic value compared between replay and inspection.
    pub compared: &'static str,
    /// Related information intentionally excluded from this comparison.
    pub ignored: &'static str,
    /// Canonicalization applied before comparison.
    pub normalization: &'static str,
    /// Additional limitation users should consider when interpreting drift.
    pub note: Option<&'static str>,
}

/// Builds the public contract entry for one registered comparator.
pub(super) fn property_doc(entity_kind: EntityKind, property: &'static str) -> DriftPropertyDoc {
    let (compared, normalization) = property_semantics(property);
    DriftPropertyDoc {
        entity_kind,
        property,
        compared,
        ignored: ignored_semantics(entity_kind),
        normalization,
        note: opaque_note(entity_kind),
    }
}

fn property_semantics(property: &str) -> (&'static str, &'static str) {
    match property {
        "name" => ("stable entity identity", "exact identifier comparison"),
        "schema" => (
            "canonical schema identity",
            "dialect schema canonicalization",
        ),
        "type" => ("canonical column type", "dialect type alias normalization"),
        "nullable" | "primary_key" | "unique" | "security_definer" | "auto_increment"
        | "invisible" => ("modeled boolean state", "exact boolean comparison"),
        "default" => (
            "modeled default expression",
            "dialect-safe literal canonicalization",
        ),
        "columns" | "to_columns" | "values" | "events" => (
            "ordered modeled values",
            "order-preserving exact comparison",
        ),
        "on_delete" | "on_update" => (
            "canonical foreign-key action",
            "canonical action keyword comparison",
        ),
        "definition" | "predicate" | "check" | "generated" | "on_update_expression" => (
            "stable inspected expression text",
            "dialect comparator normalization",
        ),
        _ => (
            "registered modeled metadata",
            "comparator-specific canonicalization",
        ),
    }
}

fn ignored_semantics(entity_kind: EntityKind) -> &'static str {
    match entity_kind {
        EntityKind::Function => "function body and opaque raw SQL",
        EntityKind::View => "view body when it is not a registered stable property",
        EntityKind::Trigger => "trigger body, WHEN source, and opaque raw SQL",
        EntityKind::Index => {
            "access method, operator classes, collation, sort metadata, and opaque raw SQL"
        }
        EntityKind::Table => "unmanaged table options and unregistered catalog metadata",
        _ => "unregistered properties and opaque raw SQL",
    }
}

fn opaque_note(entity_kind: EntityKind) -> Option<&'static str> {
    matches!(
        entity_kind,
        EntityKind::Index
            | EntityKind::Constraint
            | EntityKind::Trigger
            | EntityKind::Function
            | EntityKind::View
            | EntityKind::Extension
    )
    .then_some("opaque forms are verified by owned presence only")
}
