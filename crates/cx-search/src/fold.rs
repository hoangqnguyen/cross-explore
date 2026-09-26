use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

/// Case- and diacritic-insensitive key: "Ảnh Đẹp.JPG" → "anh dep.jpg".
///
/// Decomposes (NFD), drops combining marks and lowercases. A few letters that
/// carry their "accent" in the base character (đ, ł, ø) don't decompose, so
/// they are mapped by hand — users typing "dep" expect to find "đẹp".
/// ASCII input (the common case) takes a fast path.
pub fn fold(s: &str) -> String {
    if s.is_ascii() {
        return s.to_ascii_lowercase();
    }
    s.nfd()
        .filter(|c| !is_combining_mark(*c))
        .map(|c| match c {
            'đ' | 'Đ' => 'd',
            'ł' | 'Ł' => 'l',
            'ø' | 'Ø' => 'o',
            c => c,
        })
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::fold;

    #[test]
    fn folds_case_and_marks() {
        assert_eq!(fold("Ảnh Đẹp.JPG"), "anh dep.jpg");
        assert_eq!(fold("Crème Brûlée"), "creme brulee");
        assert_eq!(fold("README"), "readme");
    }
}
