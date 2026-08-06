use super::model::ClarificationKind;

pub fn clarification_id(kind: &ClarificationKind) -> String {
    match kind {
        ClarificationKind::RenameColumn { table, old, .. } => {
            format!("rename_col:{}:{}", encode_part(table), encode_part(old))
        }
        ClarificationKind::RenameTable { old, .. } => {
            format!("rename_table:{}", encode_part(old))
        }
        ClarificationKind::RenameEnumValue { enum_name, old, .. } => {
            format!(
                "rename_enum_value:{}:{}",
                encode_part(enum_name),
                encode_part(old)
            )
        }
        ClarificationKind::NotNullAdd { table, column, .. } => {
            format!("notnull_add:{}:{}", encode_part(table), encode_part(column))
        }
        ClarificationKind::NotNullChange { table, column } => {
            format!(
                "notnull_change:{}:{}",
                encode_part(table),
                encode_part(column)
            )
        }
        ClarificationKind::TypeCast { table, column, .. } => {
            format!("typecast:{}:{}", encode_part(table), encode_part(column))
        }
        ClarificationKind::UnknownType { table, column, .. } => {
            format!(
                "unknown_type:{}:{}",
                encode_part(table),
                encode_part(column)
            )
        }
        ClarificationKind::OpaqueEntity { kind, name } => {
            let kind = format!("{kind:?}").to_ascii_lowercase();
            format!("opaque_entity:{}:{}", kind, encode_part(name))
        }
        ClarificationKind::UnmanagedTableOptions { table } => {
            format!("unmanaged_table_options:{}", encode_part(table))
        }
        ClarificationKind::ColumnMetadataChange { table, column, .. } => {
            format!(
                "column_metadata:{}:{}",
                encode_part(table),
                encode_part(column)
            )
        }
        ClarificationKind::DeleteManagedRow { table, identity } => format!(
            "delete_managed_row:{}:{}",
            encode_part(table),
            encode_part(identity)
        ),
    }
}

pub fn is_unknown_type_id(id: &str) -> bool {
    id.starts_with("unknown_type:")
}

fn encode_part(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        let keep = byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/');
        if keep {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(hex(byte >> 4));
            encoded.push(hex(byte & 0x0f));
        }
    }
    encoded
}

fn hex(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        _ => (b'A' + (value - 10)) as char,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clarifier::model::ClarificationKind;

    #[test]
    fn simple_ids_keep_legacy_shape() {
        let id = clarification_id(&ClarificationKind::RenameColumn {
            table: "users".to_string(),
            old: "email".to_string(),
            candidates: vec!["email_address".to_string()],
        });
        assert_eq!(id, "rename_col:users:email");
    }

    #[test]
    fn ids_escape_separators_in_names() {
        let id = clarification_id(&ClarificationKind::RenameColumn {
            table: "tenant:users".to_string(),
            old: "email%old".to_string(),
            candidates: vec!["email:new".to_string()],
        });
        assert_eq!(id, "rename_col:tenant%3Ausers:email%25old");
    }
}
