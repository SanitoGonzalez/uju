//! A line-based writer that indents Rust as rustfmt would.

/// Lines of Rust, indented by brace depth when finished.
#[derive(Default)]
pub(super) struct Out {
    lines: Vec<String>,
}

impl Out {
    pub(super) fn line(&mut self, line: impl Into<String>) {
        let line = line.into();
        // Multi-line pieces, such as closures built as one string.
        for part in line.split('\n') {
            // No blank line opens a block.
            let opens = self.lines.last().is_some_and(|last| last.ends_with('{'));
            if part.is_empty() && opens {
                continue;
            }
            self.lines.push(part.to_string());
        }
    }

    pub(super) fn extend(&mut self, lines: Vec<String>) {
        for line in lines {
            self.line(line);
        }
    }

    /// Indents each line one level per line that left brackets open, as
    /// rustfmt does: `f(x, |y| {` indents its body once, not twice.
    pub(super) fn finish(self) -> String {
        let mut out = String::new();
        // How many brackets each open level still holds.
        let mut levels: Vec<usize> = Vec::new();
        for line in &self.lines {
            let line = line.trim();
            if line.is_empty() {
                out.push('\n');
                continue;
            }
            let mut indent = None;
            let mut pushed = false;
            let mut in_string = false;
            let mut escaped = false;
            for ch in line.chars() {
                if in_string {
                    match ch {
                        _ if escaped => escaped = false,
                        '\\' => escaped = true,
                        '"' => in_string = false,
                        _ => {}
                    }
                    continue;
                }
                // The first character that isn't a closer fixes the indent.
                if !matches!(ch, '}' | ')' | ']') {
                    indent.get_or_insert(levels.len());
                }
                match ch {
                    '"' => in_string = true,
                    '{' | '(' | '[' => match levels.last_mut() {
                        Some(count) if pushed => *count += 1,
                        _ => {
                            levels.push(1);
                            pushed = true;
                        }
                    },
                    '}' | ')' | ']' => {
                        if let Some(count) = levels.last_mut() {
                            *count -= 1;
                            if *count == 0 {
                                levels.pop();
                                pushed = false;
                            }
                        }
                    }
                    _ => {}
                }
            }
            let indent = indent.unwrap_or(levels.len());
            for _ in 0..indent {
                out.push_str("    ");
            }
            out.push_str(line);
            out.push('\n');
        }
        out
    }
}
