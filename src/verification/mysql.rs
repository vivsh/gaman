use super::VerificationRegistry;

pub(crate) fn registry() -> &'static VerificationRegistry {
    &REGISTRY
}

static REGISTRY: VerificationRegistry = VerificationRegistry {
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
