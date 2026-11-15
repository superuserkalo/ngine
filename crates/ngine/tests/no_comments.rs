use std::fs;
use std::path::{Path, PathBuf};

const FIX: &str = "Comments are banned. Say it in code: rename, extract a function, or encode \
the rule in a type or a test. A comment may stay only as a doc comment that states a public \
item's contract, a `// SAFETY:` note on an unsafe block, or a constraint forced by an operating \
system, vendor or protocol that links its source with https://.";

struct Comment {
    line: usize,
    last_line: usize,
    text: String,
    mergeable: bool,
}

struct Scanner {
    chars: Vec<char>,
    at: usize,
    line: usize,
    found: Vec<Comment>,
}

impl Scanner {
    fn new(source: &str) -> Self {
        Self {
            chars: source.chars().collect(),
            at: 0,
            line: 1,
            found: Vec::new(),
        }
    }

    fn peek(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.at + ahead).copied()
    }

    fn advance(&mut self) {
        if self.peek(0) == Some('\n') {
            self.line += 1;
        }
        self.at += 1;
    }

    fn run(mut self) -> Vec<Comment> {
        while let Some(current) = self.peek(0) {
            match (current, self.peek(1)) {
                ('/', Some('/')) => self.line_comment(),
                ('/', Some('*')) => self.block_comment(),
                ('"', _) => self.string(),
                ('r', Some('"' | '#')) => self.raw_string(),
                ('\'', _) => self.char_or_lifetime(),
                _ => self.advance(),
            }
        }
        self.found
    }

    fn line_comment(&mut self) {
        let line = self.line;
        let mut text = String::new();
        while let Some(current) = self.peek(0) {
            if current == '\n' {
                break;
            }
            text.push(current);
            self.at += 1;
        }
        let doc = text.starts_with("///") || text.starts_with("//!");
        if let Some(last) = self.found.last_mut()
            && last.mergeable
            && !doc
            && last.last_line + 1 == line
        {
            last.text.push('\n');
            last.text.push_str(&text);
            last.last_line = line;
        } else {
            self.found.push(Comment {
                line,
                last_line: line,
                text,
                mergeable: !doc,
            });
        }
    }

    fn block_comment(&mut self) {
        let line = self.line;
        let start = self.at;
        let mut depth = 0_usize;
        loop {
            match (self.peek(0), self.peek(1)) {
                (Some('/'), Some('*')) => {
                    depth += 1;
                    self.at += 2;
                }
                (Some('*'), Some('/')) => {
                    depth -= 1;
                    self.at += 2;
                    if depth == 0 {
                        break;
                    }
                }
                (Some(_), _) => self.advance(),
                (None, _) => break,
            }
        }
        self.found.push(Comment {
            line,
            last_line: self.line,
            text: self.chars[start..self.at].iter().collect(),
            mergeable: false,
        });
    }

    fn string(&mut self) {
        self.at += 1;
        while let Some(current) = self.peek(0) {
            match current {
                '\\' => {
                    self.advance();
                    self.advance();
                }
                '"' => {
                    self.at += 1;
                    return;
                }
                _ => self.advance(),
            }
        }
    }

    fn raw_string(&mut self) {
        let mut hashes = 0;
        while self.peek(1 + hashes) == Some('#') {
            hashes += 1;
        }
        if self.peek(1 + hashes) != Some('"') {
            self.advance();
            return;
        }
        self.at += 2 + hashes;
        while self.peek(0).is_some() {
            let closes = self.peek(0) == Some('"')
                && (1..=hashes).all(|offset| self.peek(offset) == Some('#'));
            if closes {
                self.at += 1 + hashes;
                return;
            }
            self.advance();
        }
    }

    fn char_or_lifetime(&mut self) {
        if self.peek(1) == Some('\\') {
            self.at += 3;
            while let Some(current) = self.peek(0) {
                self.at += 1;
                if current == '\'' {
                    return;
                }
            }
        } else if self.peek(2) == Some('\'') {
            self.at += 3;
        } else {
            self.at += 1;
        }
    }
}

fn allowed(text: &str) -> bool {
    let doc = ["///", "//!", "/**", "/*!"]
        .iter()
        .any(|start| text.starts_with(start));
    doc || text.starts_with("// SAFETY:") || text.contains("https://")
}

fn rust_files(directory: &Path, found: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(directory).expect("source directory is readable");
    for entry in entries {
        let path = entry.expect("directory entry is readable").path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name != "target") {
                rust_files(&path, found);
            }
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

#[test]
fn source_has_no_comments() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate lives two levels below the workspace root");
    let mut files = Vec::new();
    rust_files(&root.join("crates"), &mut files);
    assert!(!files.is_empty(), "no Rust files found under crates/");

    let mut banned = Vec::new();
    for file in &files {
        let source = fs::read_to_string(file).expect("source file is readable");
        let shown = file.strip_prefix(root).unwrap_or(file).display();
        for comment in Scanner::new(&source).run() {
            if !allowed(&comment.text) {
                let first_line = comment.text.lines().next().unwrap_or_default();
                banned.push(format!("{shown}:{}: {first_line}", comment.line));
            }
        }
    }
    assert!(banned.is_empty(), "{FIX}\n\n{}\n", banned.join("\n"));
}

#[test]
fn scanner_tells_comments_from_code() {
    let code = "fn f<'a>(s: &'a str) -> char { let _u = \"http://a//b\"; \
                let _r = r#\"// \"quoted\" /* no */\"#; let _q = '\\''; '/' }";
    assert!(Scanner::new(code).run().is_empty());

    let commented = "let a = 1; // one\n// two\nlet b = 2;\n/* three\n/* nested */ */";
    let found = Scanner::new(commented).run();
    let texts: Vec<&str> = found.iter().map(|comment| comment.text.as_str()).collect();
    assert_eq!(texts, ["// one\n// two", "/* three\n/* nested */ */"]);
    assert_eq!(found[1].line, 4);
}

#[test]
fn only_doc_safety_and_sourced_comments_are_allowed() {
    assert!(allowed("/// Returns the frame's timestamp."));
    assert!(allowed("// SAFETY: the pointer is valid for the call."));
    assert!(allowed(
        "// Wayland hides the cursor here: https://example.org/spec"
    ));
    assert!(!allowed("// loop over the frames"));
    assert!(!allowed("/* temporary workaround */"));
}
