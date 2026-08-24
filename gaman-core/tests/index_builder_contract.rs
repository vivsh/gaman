use gaman_core::dialects::Dialect;
use gaman_core::schema::{Index, SchemaBuilder, TableBuilder};

/// Verifies external embedders can compose modeled and opaque indexes through distinct APIs.
#[test]
fn modeled_and_opaque_index_declarations_remain_composable() {
    let articles = TableBuilder::new("articles")
        .column("tenant_id", "integer", |column| column)
        .column("slug", "text", |column| column)
        .index(
            Index::columns(["tenant_id", "slug"])
                .named("articles_tenant_slug_idx")
                .unique(),
        )
        .build();
    let schema = SchemaBuilder::new(Dialect::Postgres)
        .table_def(articles)
        .opaque("CREATE INDEX articles_slug_lower_idx ON articles ((lower(slug)))")
        .build()
        .expect("valid modeled and opaque index schema");

    let indexes = &schema.tables["articles"].indexes;
    assert_eq!(indexes.len(), 2);
    assert!(indexes.iter().any(|index| {
        index.name == "articles_tenant_slug_idx" && index.unique && !index.is_opaque()
    }));
    assert!(
        indexes
            .iter()
            .any(|index| index.name == "articles_slug_lower_idx" && index.is_opaque())
    );
}
