//! Total Commander's select-by-pattern wildcards: `*.jpg;*.png`, `IMG_????`,
//! case-insensitive, whole-name. Several patterns are separated by `;` or `,`.

/// A compiled pattern list.
#[derive(Debug, Clone)]
pub struct Wildcards(Vec<Vec<char>>);

impl Wildcards {
    pub fn new(pattern: &str) -> Wildcards {
        Wildcards(
            pattern
                .split([';', ','])
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(|p| p.to_lowercase().chars().collect())
                .collect(),
        )
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn matches(&self, name: &str) -> bool {
        let name: Vec<char> = name.to_lowercase().chars().collect();
        self.0.iter().any(|p| glob(p, &name))
    }
}

/// Iterative wildcard match with backtracking to the last `*`.
fn glob(p: &[char], s: &[char]) -> bool {
    let (mut pi, mut si) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while si < s.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, si));
            pi += 1;
        } else if let Some((sp, ss)) = star {
            pi = sp + 1;
            si = ss + 1;
            star = Some((sp, ss + 1));
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards() {
        let w = Wildcards::new("*.JPG; img_????.png");
        assert!(w.matches("photo.jpg"));
        assert!(w.matches("IMG_1234.png"));
        assert!(!w.matches("IMG_12345.png"));
        assert!(!w.matches("photo.jpeg"));
        assert!(Wildcards::new("*").matches("anything"));
        assert!(Wildcards::new("a*b*c").matches("aXXbYYc"));
        assert!(!Wildcards::new("a*b*c").matches("aXXbYY"));
        assert!(Wildcards::new("").is_empty());
    }
}
