use super::DriftRegistry;

pub(crate) fn registry() -> &'static DriftRegistry {
    &REGISTRY
}

static REGISTRY: DriftRegistry = DriftRegistry {
    tables: &[],
    columns: &[],
    primary_keys: &[],
    foreign_keys: &[],
    indexes: &[],
    constraints: &[],
    triggers: &[],
    functions: &[],
    views: &[],
    enums: &[],
    extensions: &[],
};
