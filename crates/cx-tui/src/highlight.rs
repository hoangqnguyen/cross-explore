//! Just-enough syntax highlighting for previews: comments, strings,
//! numbers and keywords for the common language families, headings and
//! code in Markdown. A line-by-line lexer (block comments carry over), so a
//! 500 KB file highlights in a few milliseconds and never blocks a frame.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tok {
    Plain,
    Keyword,
    Str,
    Comment,
    Number,
    Heading,
    Code,
    Punct,
}

struct Lang {
    line_comments: &'static [&'static str],
    block: Option<(&'static str, &'static str)>,
    keywords: &'static [&'static str],
    backtick_strings: bool,
}

const C_LIKE: &[&str] = &[
    "fn", "let", "mut", "pub", "use", "mod", "struct", "enum", "impl", "trait", "match", "if", "else", "for", "while", "loop", "return", "break", "continue", "const", "static", "async", "await", "move", "where", "type", "self", "Self", "true", "false", "function", "var", "class", "new", "this", "import", "export", "from", "default", "extends", "interface", "public", "private", "protected", "void", "int", "char", "float", "double", "bool", "boolean", "string", "null", "undefined", "try", "catch", "finally", "throw", "throws", "switch", "case", "package", "func", "go", "defer", "chan", "select", "struct", "namespace", "using", "virtual", "override", "unsafe", "extern", "crate", "super", "in", "of", "typeof", "instanceof", "yield", "val", "fun", "object", "when", "is", "as", "do", "nil", "None", "Some", "Ok", "Err",
];
const PY_LIKE: &[&str] = &[
    "def", "class", "import", "from", "as", "if", "elif", "else", "for", "while", "return", "yield", "with", "try", "except", "finally", "raise", "lambda", "pass", "break", "continue", "and", "or", "not", "in", "is", "None", "True", "False", "global", "nonlocal", "async", "await", "self", "end", "do", "then", "fi", "done", "esac", "case", "function", "local", "export", "echo", "module", "require", "puts", "unless", "elsif", "begin", "rescue", "ensure",
];
const SQL: &[&str] = &["select", "from", "where", "insert", "into", "values", "update", "set", "delete", "create", "table", "index", "join", "left", "right", "inner", "outer", "on", "group", "by", "order", "having", "limit", "and", "or", "not", "null", "as", "distinct", "union", "primary", "key", "SELECT", "FROM", "WHERE", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "CREATE", "TABLE", "JOIN", "ON", "GROUP", "BY", "ORDER", "AND", "OR", "NOT", "NULL", "AS", "LIMIT"];

fn lang(name: &str) -> Option<Lang> {
    let l = match name {
        "rust" | "c" | "cpp" | "c++" | "java" | "javascript" | "typescript" | "go" | "swift" | "kotlin" | "csharp" | "c#" | "scala" | "dart" | "php" | "objective-c" | "objectivec" | "protobuf" | "zig" | "json" | "jsx" | "tsx" | "svelte" | "vue" | "css" | "scss" | "less" | "groovy" => Lang { line_comments: &["//"], block: Some(("/*", "*/")), keywords: C_LIKE, backtick_strings: matches!(name, "javascript" | "typescript" | "jsx" | "tsx" | "svelte" | "vue" | "go") },
        "python" | "ruby" | "shell" | "bash" | "sh" | "zsh" | "fish" | "perl" | "r" | "yaml" | "toml" | "makefile" | "dockerfile" | "powershell" | "nim" | "elixir" | "ini" | "conf" | "cmake" | "graphql" => Lang { line_comments: &["#"], block: None, keywords: PY_LIKE, backtick_strings: false },
        "lua" | "haskell" | "sql" => Lang { line_comments: &["--"], block: None, keywords: if name == "sql" { SQL } else { PY_LIKE }, backtick_strings: false },
        "html" | "xml" | "svg" => Lang { line_comments: &[], block: Some(("<!--", "-->")), keywords: &[], backtick_strings: false },
        _ => return None,
    };
    Some(l)
}

pub type Line = Vec<(Tok, String)>;

fn push(out: &mut Line, tok: Tok, s: &str) {
    if s.is_empty() {
        return;
    }
    if let Some(last) = out.last_mut() {
        if last.0 == tok {
            last.1.push_str(s);
            return;
        }
    }
    out.push((tok, s.to_string()));
}

/// Highlight `text` (expected to be a preview, not a whole huge file) for
/// `language` (a `cx_thumbs::language_for_name` name). Unknown languages
/// come back as plain lines.
pub fn highlight(text: &str, language: Option<&str>) -> Vec<Line> {
    let language = language.map(|l| l.to_ascii_lowercase());
    if matches!(language.as_deref(), Some("markdown") | Some("md")) {
        return markdown(text);
    }
    let Some(lang) = language.as_deref().and_then(lang) else {
        return text.lines().map(|l| vec![(Tok::Plain, l.to_string())]).collect();
    };
    let mut in_block = false;
    text.lines().map(|l| code_line(l, &lang, &mut in_block)).collect()
}

fn code_line(line: &str, lang: &Lang, in_block: &mut bool) -> Line {
    let mut out = Line::new();
    let mut i = 0;
    let b = line.as_bytes();
    while i < line.len() {
        let rest = &line[i..];
        if *in_block {
            let end = lang.block.map(|(_, e)| e).unwrap_or("");
            match rest.find(end) {
                Some(j) => {
                    push(&mut out, Tok::Comment, &rest[..j + end.len()]);
                    i += j + end.len();
                    *in_block = false;
                }
                None => {
                    push(&mut out, Tok::Comment, rest);
                    break;
                }
            }
            continue;
        }
        if lang.line_comments.iter().any(|c| rest.starts_with(c)) {
            push(&mut out, Tok::Comment, rest);
            break;
        }
        if let Some((start, _)) = lang.block {
            if rest.starts_with(start) {
                *in_block = true;
                push(&mut out, Tok::Comment, start);
                i += start.len();
                continue;
            }
        }
        let c = b[i];
        if c == b'"' || c == b'\'' || (c == b'`' && lang.backtick_strings) {
            // Apostrophes in Rust lifetimes ('a) would swallow the line;
            // treat a quote as a string only if it closes on this line.
            let mut j = i + 1;
            let mut closed = false;
            while j < b.len() {
                if b[j] == b'\\' {
                    j += 2;
                    continue;
                }
                if b[j] == c {
                    closed = true;
                    break;
                }
                j += 1;
            }
            if closed {
                let end = (j + 1).min(line.len());
                if line.is_char_boundary(end) {
                    push(&mut out, Tok::Str, &line[i..end]);
                    i = end;
                    continue;
                }
            }
        }
        if c.is_ascii_digit() {
            let mut j = i;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'.' || b[j] == b'_') {
                j += 1;
            }
            push(&mut out, Tok::Number, &line[i..j]);
            i = j;
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let mut j = i;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            let word = &line[i..j];
            push(&mut out, if lang.keywords.contains(&word) { Tok::Keyword } else { Tok::Plain }, word);
            i = j;
            continue;
        }
        let ch = rest.chars().next().unwrap();
        let tok = if "{}()[];,.:<>=+-*/&|!?%^~@#".contains(ch) { Tok::Punct } else { Tok::Plain };
        let mut buf = [0u8; 4];
        push(&mut out, tok, ch.encode_utf8(&mut buf));
        i += ch.len_utf8();
    }
    out
}

fn markdown(text: &str) -> Vec<Line> {
    let mut fenced = false;
    text.lines()
        .map(|l| {
            if l.trim_start().starts_with("```") {
                fenced = !fenced;
                return vec![(Tok::Code, l.to_string())];
            }
            if fenced {
                return vec![(Tok::Code, l.to_string())];
            }
            if l.starts_with('#') {
                return vec![(Tok::Heading, l.to_string())];
            }
            // Inline `code` spans.
            let mut out = Line::new();
            let mut code = false;
            for (k, part) in l.split('`').enumerate() {
                if k > 0 {
                    code = !code;
                }
                push(&mut out, if code { Tok::Code } else { Tok::Plain }, part);
            }
            out
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(l: &Line) -> Vec<(Tok, &str)> {
        l.iter().map(|(t, s)| (*t, s.as_str())).collect()
    }

    #[test]
    fn rust_line() {
        let lines = highlight("let x = \"hi\"; // note\n/* a\nb */ fn", Some("rust"));
        let l0 = toks(&lines[0]);
        assert_eq!(l0[0], (Tok::Keyword, "let"));
        assert!(l0.contains(&(Tok::Str, "\"hi\"")));
        assert_eq!(l0.last().unwrap(), &(Tok::Comment, "// note"));
        assert_eq!(toks(&lines[1]), vec![(Tok::Comment, "/* a")]);
        assert_eq!(toks(&lines[2])[0], (Tok::Comment, "b */"));
        assert_eq!(toks(&lines[2]).last().unwrap(), &(Tok::Keyword, "fn"));
    }

    #[test]
    fn lifetimes_do_not_start_strings() {
        let l = highlight("fn f<'a>(x: &'a str) -> 42", Some("rust"));
        assert!(toks(&l[0]).contains(&(Tok::Number, "42")));
    }

    #[test]
    fn markdown_and_plain() {
        let l = highlight("# Title\nuse `x` here\n```\ncode\n```", Some("markdown"));
        assert_eq!(l[0][0].0, Tok::Heading);
        assert_eq!(toks(&l[1]), vec![(Tok::Plain, "use "), (Tok::Code, "x"), (Tok::Plain, " here")]);
        assert_eq!(l[3][0].0, Tok::Code);
        assert_eq!(highlight("a\nb", None).len(), 2);
    }
}
