use once_cell::sync::Lazy;

static DELIMS: Lazy<[char; 6]> = Lazy::new(|| [' ', '\n', '\t', ',', ';', '.']);

/// Split a string into lowercase words using a small set of delimiters.
pub fn split_words(input: &str) -> Vec<String> {
    input
        .split(|ch| DELIMS.contains(&ch))
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .collect()
}

/// Join words with a custom separator and optional trailing terminator.
pub fn join_words(words: &[impl AsRef<str>], sep: &str, term: Option<&str>) -> String {
    let mut out = words.iter().map(|w| w.as_ref()).collect::<Vec<_>>().join(sep);
    if let Some(t) = term { out.push_str(t); }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_and_join() {
        let words = split_words("Hello, world. hello");
        assert_eq!(words, vec!["hello".to_string(), "world".to_string(), "hello".to_string()]);
        let j = join_words(&words, "-", Some("."));
        assert_eq!(j, "hello-world-hello.");
    }
}


