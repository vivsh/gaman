pub fn normalize_type(t: &str) -> String {
    let t = t.trim().to_lowercase();
    match t.find('(') {
        Some(idx) => t[..idx].trim_end().to_string(),
        None => t,
    }
}

fn type_family(t: &str) -> Option<u8> {
    match t {
        "text" | "varchar" | "char" | "character varying" | "character" | "bpchar" | "name" => {
            Some(0)
        }
        "int" | "integer" | "int4" | "int8" | "int2" | "bigint" | "smallint" | "serial"
        | "bigserial" | "smallserial" => Some(1),
        "float" | "float4" | "float8" | "real" | "double precision" => Some(2),
        "numeric" | "decimal" => Some(3),
        "bool" | "boolean" => Some(4),
        _ => None,
    }
}

pub fn types_compatible(a: &str, b: &str) -> bool {
    let na = normalize_type(a);
    let nb = normalize_type(b);
    if na == nb {
        return true;
    }
    match (type_family(&na), type_family(&nb)) {
        (Some(fa), Some(fb)) => fa == fb,
        _ => false,
    }
}
