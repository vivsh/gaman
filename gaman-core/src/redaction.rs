//! Secret-safe rendering helpers for user-facing diagnostics.

/// Redacts database URL passwords and common assignment-style secrets from diagnostic text.
pub fn redact_diagnostic_text(value: &str) -> String {
    redact_assignments(&redact_url_userinfo(value))
}

/// Replaces password-bearing URL user-info while preserving useful endpoint context.
fn redact_url_userinfo(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut copied_from = 0;
    let mut scan_from = 0;
    while let Some(relative) = value[scan_from..].find("://") {
        let authority_start = scan_from + relative + 3;
        let authority_end = authority_end(value, authority_start);
        if let Some((password_start, password_end)) = password_range(
            &value[authority_start..authority_end],
            authority_start,
        ) {
            output.push_str(&value[copied_from..password_start]);
            output.push_str("***");
            copied_from = password_end;
        }
        scan_from = authority_end;
    }
    output.push_str(&value[copied_from..]);
    output
}

/// Returns the end of a URL authority, including an authority that reaches end-of-string.
fn authority_end(value: &str, authority_start: usize) -> usize {
    value[authority_start..]
        .char_indices()
        .find_map(|(offset, character)| {
            (character.is_whitespace() || matches!(character, '/' | '?' | '#'))
                .then_some(authority_start + offset)
        })
        .unwrap_or(value.len())
}

/// Returns the absolute range occupied by a URL password, when user-info contains one.
fn password_range(authority: &str, offset: usize) -> Option<(usize, usize)> {
    let at = authority.rfind('@')?;
    let colon = authority[..at].find(':')?;
    Some((offset + colon + 1, offset + at))
}

/// Redacts values assigned to secret-like keys without matching longer ordinary identifiers.
fn redact_assignments(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut copied_from = 0;
    let mut scan_from = 0;
    while let Some((value_start, allow_spaces)) = secret_value_start(value, scan_from) {
        let value_end = secret_value_end(value, value_start, allow_spaces);
        output.push_str(&value[copied_from..value_start]);
        output.push_str("***");
        copied_from = value_end;
        scan_from = value_end;
    }
    output.push_str(&value[copied_from..]);
    output
}

/// Finds one secret assignment and returns its value start plus whitespace policy.
fn secret_value_start(value: &str, scan_from: usize) -> Option<(usize, bool)> {
    const KEYS: [(&str, bool); 8] = [
        ("access_token", false),
        ("authorization", true),
        ("password", false),
        ("api_key", false),
        ("apikey", false),
        ("secret", false),
        ("token", false),
        ("pwd", false),
    ];

    for (offset, _) in value[scan_from..].char_indices() {
        let start = scan_from + offset;
        if !assignment_boundary(value, start) {
            continue;
        }
        for (key, allow_spaces) in KEYS {
            let after_key = start + key.len();
            if !value[start..].starts_with(key) && !value[start..].to_ascii_lowercase().starts_with(key) {
                continue;
            }
            let equals = value[after_key..]
                .char_indices()
                .find_map(|(offset, character)| (!character.is_whitespace()).then_some(after_key + offset))?;
            if value.as_bytes().get(equals) != Some(&b'=') {
                continue;
            }
            let value_start = value[equals + 1..]
                .char_indices()
                .find_map(|(offset, character)| (!character.is_whitespace()).then_some(equals + 1 + offset))?;
            if assignment_delimiter(value[value_start..].chars().next()?) {
                continue;
            }
            return Some((value_start, allow_spaces));
        }
    }
    None
}

/// Checks that a candidate assignment key is not part of a longer identifier.
fn assignment_boundary(value: &str, start: usize) -> bool {
    value[..start]
        .chars()
        .next_back()
        .is_none_or(|character| !(character.is_ascii_alphanumeric() || matches!(character, '_' | '-')))
}

/// Returns the exclusive end of one secret value, including matching quote delimiters.
fn secret_value_end(value: &str, start: usize, allow_spaces: bool) -> usize {
    let Some(first) = value[start..].chars().next() else {
        return start;
    };
    if matches!(first, '\'' | '"') {
        return quoted_value_end(value, start, first);
    }
    value[start..]
        .char_indices()
        .find_map(|(offset, character)| {
            (assignment_delimiter(character) || (!allow_spaces && character.is_whitespace()))
                .then_some(start + offset)
        })
        .unwrap_or(value.len())
}

/// Returns the exclusive end of a quoted secret value while honoring backslash escapes.
fn quoted_value_end(value: &str, start: usize, quote: char) -> usize {
    let mut escaped = false;
    for (offset, character) in value[start + quote.len_utf8()..].char_indices() {
        if !escaped && character == quote {
            return start + quote.len_utf8() + offset + quote.len_utf8();
        }
        escaped = !escaped && character == '\\';
    }
    value.len()
}

/// Identifies punctuation that terminates an unquoted secret value.
fn assignment_delimiter(character: char) -> bool {
    matches!(character, '&' | ',' | ';' | '#' | ')' | ']' | '}')
}

#[cfg(test)]
mod tests {
    use super::redact_diagnostic_text;

    /// Verifies diagnostic redaction covers supported URL and key-value secret forms.
    #[test]
    fn redacts_url_and_assignment_secrets() {
        let cases = [
            (
                "postgres://user:secret@host",
                "postgres://user:***@host",
                "secret",
            ),
            (
                "mysql://user:secret@host/app?token=abc#part",
                "mysql://user:***@host/app?token=***#part",
                "secret",
            ),
            (
                "mariadb://user:pa%3Ass@host/a postgres://a:b@h/c",
                "mariadb://user:***@host/a postgres://a:***@h/c",
                "pa%3Ass",
            ),
            (
                "PASSWORD = 'secret value' api_key=abc, Access_Token=xyz",
                "PASSWORD = *** api_key=***, Access_Token=***",
                "secret value",
            ),
            (
                "authorization=Bearer secret; pwd=hidden token=next",
                "authorization=***; pwd=*** token=***",
                "secret",
            ),
        ];

        for (input, expected, secret) in cases {
            let actual = redact_diagnostic_text(input);
            assert_eq!(actual, expected);
            assert!(!actual.contains(secret));
        }
    }

    /// Verifies benign identifiers and SQLite connection strings remain readable.
    #[test]
    fn preserves_non_secret_text() {
        assert_eq!(
            redact_diagnostic_text("sqlite::memory: notpassword=value user@host"),
            "sqlite::memory: notpassword=value user@host"
        );
    }
}
