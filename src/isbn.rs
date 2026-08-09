//! ISBN-10/13 checksum validation and normalization to ISBN-13.

/// Strip hyphens and spaces. Returns the cleaned candidate only if the result
/// is shaped like an ISBN (10 or 13 characters of the right alphabet).
pub fn looks_like_isbn(input: &str) -> Option<String> {
    let cleaned: String = input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    match cleaned.len() {
        10 => {
            let (digits, check) = cleaned.split_at(9);
            (digits.chars().all(|c| c.is_ascii_digit())
                && check.chars().all(|c| c.is_ascii_digit() || c == 'X'))
            .then_some(cleaned)
        }
        13 => cleaned.chars().all(|c| c.is_ascii_digit()).then_some(cleaned),
        _ => None,
    }
}

/// Validate checksum and normalize to ISBN-13. Input must already be cleaned
/// (output of [`looks_like_isbn`]).
pub fn normalize(cleaned: &str) -> Option<String> {
    match cleaned.len() {
        10 if valid_isbn10(cleaned) => Some(isbn10_to_13(cleaned)),
        13 if valid_isbn13(cleaned) => Some(cleaned.to_string()),
        _ => None,
    }
}

fn valid_isbn10(isbn: &str) -> bool {
    let sum: u32 = isbn.chars().enumerate().fold(0, |acc, (i, c)| {
        let value = if c == 'X' { 10 } else { c.to_digit(10).unwrap_or(0) };
        acc + value * (10 - i as u32)
    });
    sum.is_multiple_of(11)
}

fn valid_isbn13(isbn: &str) -> bool {
    let sum: u32 = isbn.chars().enumerate().fold(0, |acc, (i, c)| {
        let value = c.to_digit(10).unwrap_or(0);
        acc + value * if i % 2 == 0 { 1 } else { 3 }
    });
    sum.is_multiple_of(10)
}

fn isbn10_to_13(isbn10: &str) -> String {
    let body = format!("978{}", &isbn10[..9]);
    let sum: u32 = body.chars().enumerate().fold(0, |acc, (i, c)| {
        acc + c.to_digit(10).unwrap() * if i % 2 == 0 { 1 } else { 3 }
    });
    format!("{body}{}", (10 - sum % 10) % 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_isbn_shapes() {
        assert_eq!(looks_like_isbn("978-0-13-468599-1"), Some("9780134685991".into()));
        assert_eq!(looks_like_isbn("0-306-40615-2"), Some("0306406152".into()));
        assert_eq!(looks_like_isbn("043942089x"), Some("043942089X".into()));
        assert_eq!(looks_like_isbn("The Rust Book"), None);
        assert_eq!(looks_like_isbn("12345"), None);
    }

    #[test]
    fn validates_checksums() {
        assert_eq!(normalize("9780134685991"), Some("9780134685991".into()));
        assert_eq!(normalize("9780134685992"), None);
        assert_eq!(normalize("0306406152"), Some("9780306406157".into()));
        assert_eq!(normalize("0306406153"), None);
    }

    #[test]
    fn converts_isbn10_with_x_check() {
        // 043942089X = Harry Potter and the Goblet of Fire (US hardcover)
        assert_eq!(normalize("043942089X"), Some("9780439420891".into()));
    }
}
