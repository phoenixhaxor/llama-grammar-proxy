//! Smart comment stripping filter for code blocks inside LLM message content.
//!
//! SAFETY PRINCIPLES:
//! - Only operates INSIDE markdown code blocks (```...```)
//! - Never touches plain text, tool results, or terminal output outside code blocks
//! - When in doubt, DON'T strip. False negatives > false positives.
//! - All stripping is opt-out via --no-filter flag
//!
//! Strategy per language:
//!   Rust:       // and /// and /** */
//!   Python:     # and """ """
//!   Go:         // and /* */
//!   JavaScript/TypeScript: // and /* */
//!   Shell/Bash: # only
//!   C/C++/Java: // and /* */

use regex::Regex;
use std::collections::HashSet;
use once_cell::sync::Lazy;

// ── Keywords that indicate a "why" comment — KEEP these ──────────────

static WHY_KEYWORDS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    HashSet::from([
        "because", "due to", "workaround", "hack", "todo", "fixme",
        "important", "note", "bug", "constraint", "warning", "caution",
        "must", "required", "cannot", "don't", "do not", "necessary",
        "critical", "unsafe", "careful", "beware", "ensure", "remember",
        "nb", "n.b.", "xxx", "perf", "security", "race condition",
        "deadlock", "memory leak", "overflow", "undefined", "compatibility",
        "breaking", "deprecated", "legacy", "temporal", "ordering",
        "side effect", "gotcha", "pitfall", "subtle", "non-obvious",
        "intentional", "deliberate", "on purpose", "by design",
    ])
});

// ── Comment prefix patterns to KEEP (todo/fixme/hack etc.) ──────────

static KEEP_PREFIXES: Lazy<Vec<&'static str>> = Lazy::new(|| {
    vec![
        "todo:", "fixme:", "hack:", "xxx:", "note:", "warning:",
        "bug:", "deprecated:", "safety:", "safety ", "unsafe:",
        "performance:", "security:", "compat:", "breaking:",
    ]
});

/// Supported languages for comment stripping
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Language {
    Rust,
    Python,
    Go,
    JavaScript,
    TypeScript,
    C,
    Cpp,
    Java,
    Shell,
    // Languages where we DON'T strip (data formats, markup, unknown)
    NoStrip,
}

impl Language {
    pub fn from_tag(tag: &str) -> Self {
        match tag.to_lowercase().trim() {
            "rust" | "rs" => Language::Rust,
            "python" | "py" | "python3" => Language::Python,
            "go" | "golang" => Language::Go,
            "javascript" | "js" | "jsx" | "mjs" | "cjs" => Language::JavaScript,
            "typescript" | "ts" | "tsx" => Language::TypeScript,
            "c" => Language::C,
            "cpp" | "c++" | "cc" | "cxx" | "hpp" => Language::Cpp,
            "java" => Language::Java,
            "sh" | "bash" | "zsh" | "shell" => Language::Shell,
            // Everything else: don't strip
            _ => Language::NoStrip,
        }
    }

    /// Returns (line_comment_prefix, has_block_comments)
    fn comment_config(&self) -> Option<(&'static str, bool)> {
        match self {
            Language::Rust | Language::Go | Language::C | Language::Cpp 
            | Language::Java | Language::JavaScript | Language::TypeScript => {
                Some(("//", true))
            }
            Language::Python | Language::Shell => {
                Some(("#", false))
            }
            Language::NoStrip => None,
        }
    }
}

/// Result of filtering a message
pub struct FilterResult {
    pub filtered_content: String,
    pub comments_stripped: usize,
    pub chars_saved: usize,
}

/// Main entry: filter all code blocks in a string
pub fn filter_message(content: &str) -> FilterResult {
    let mut result = String::with_capacity(content.len());
    let mut total_stripped = 0usize;
    let mut total_chars_saved = 0usize;
    
    let mut in_code_block = false;
    let mut code_block_tag = String::new();
    let mut code_block_lines: Vec<String> = Vec::new();
    
    for line in content.lines() {
        let trimmed = line.trim();
        
        if !in_code_block {
            // Check for code block opening
            if trimmed.starts_with("```") {
                in_code_block = true;
                code_block_tag = trimmed.trim_start_matches('`').to_string();
                code_block_lines.clear();
                result.push_str(line);
                result.push('\n');
                continue;
            }
            // Outside code block: pass through untouched
            result.push_str(line);
            result.push('\n');
        } else {
            // Inside code block
            if trimmed.starts_with("```") && trimmed.len() <= 3 {
                // End of code block — filter accumulated lines
                let lang = Language::from_tag(&code_block_tag);
                let original_len: usize = code_block_lines.iter().map(|l| l.len() + 1).sum();
                
                let filtered = filter_code_block(&code_block_lines, lang);
                let filtered_len: usize = filtered.len() + 1; // +1 for newline per line
                
                total_chars_saved += original_len.saturating_sub(filtered_len);
                
                for filtered_line in &filtered {
                    result.push_str(filtered_line);
                    result.push('\n');
                }
                result.push_str(line); // the closing ```
                result.push('\n');
                
                in_code_block = false;
                code_block_tag.clear();
                code_block_lines.clear();
            } else {
                code_block_lines.push(line.to_string());
            }
        }
    }
    
    // Handle unclosed code block (shouldn't happen but be safe)
    if in_code_block {
        for line in &code_block_lines {
            result.push_str(line);
            result.push('\n');
        }
    }
    
    // Remove trailing newline if original didn't have one
    if !content.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }
    
    FilterResult {
        filtered_content: result,
        comments_stripped: total_stripped,
        chars_saved: total_chars_saved,
    }
}

/// Filter comments from a code block
fn filter_code_block(lines: &[String], lang: Language) -> Vec<String> {
    let config = match lang.comment_config() {
        Some(c) => c,
        None => return lines.to_vec(), // Don't touch unknown languages
    };
    
    let (line_prefix, _has_block) = config;
    let mut result: Vec<String> = Vec::with_capacity(lines.len());
    let mut consecutive_blank = 0usize;
    
    for line in lines {
        let trimmed = line.trim();
        
        // ── Blank line handling: collapse to max 1 consecutive ──
        if trimmed.is_empty() {
            consecutive_blank += 1;
            if consecutive_blank <= 1 {
                result.push(line.clone());
            }
            continue;
        }
        consecutive_blank = 0;
        
        // ── Check if this line is a comment ──
        if is_comment_line(trimmed, line_prefix) {
            // SAFETY CHECKS — keep if any of these are true:
            
            // 1. Keep if it has a "keep prefix" (TODO:, FIXME:, etc.)
            let comment_body = get_comment_body(trimmed, line_prefix);
            if has_keep_prefix(&comment_body) {
                result.push(line.clone());
                continue;
            }
            
            // 2. Keep if it contains a "why" keyword
            if has_why_keyword(&comment_body) {
                result.push(line.clone());
                continue;
            }
            
            // 3. Keep if it contains a URL
            if comment_body.contains("://") || comment_body.contains("http:") || comment_body.contains("https:") {
                result.push(line.clone());
                continue;
            }
            
            // 4. Keep if it contains a version/reference number pattern
            if has_version_or_reference(&comment_body) {
                result.push(line.clone());
                continue;
            }
            
            // 5. Keep if it's a long comment (>100 chars = likely explanation)
            if comment_body.len() > 100 {
                result.push(line.clone());
                continue;
            }
            
            // 6. Keep if it's inside a string literal (check for quotes)
            // This is a heuristic — if the line has an odd number of unescaped quotes
            // before the comment, we might be inside a string
            if looks_like_string_content(line, line_prefix) {
                result.push(line.clone());
                continue;
            }
            
            // 7. Keep doc comments that have substantial content (>50 chars after prefix)
            if is_doc_comment(trimmed, line_prefix) && comment_body.len() > 50 {
                result.push(line.clone());
                continue;
            }
            
            // SAFE TO STRIP: short doc comment or separator
            continue; // Skip this line (strip it)
        }
        
        // ── Separator lines: strip decorative ones ──
        if is_separator_line(trimmed, line_prefix) {
            continue; // Strip decorative separators
        }
        
        // Not a comment — keep the line
        result.push(line.clone());
    }
    
    // Don't end with trailing blank lines
    while result.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
        result.pop();
    }
    
    result
}

/// Check if a line is a comment line for the given prefix
fn is_comment_line(trimmed: &str, prefix: &str) -> bool {
    // Must start with the prefix (after any whitespace — we already trimmed)
    if !trimmed.starts_with(prefix) {
        return false;
    }
    
    // Special case: for "//" prefix, exclude URLs
    if prefix == "//" {
        // Don't treat URLs as comments
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            return false;
        }
    }
    
    true
}

/// Extract the comment body (text after the comment prefix)
fn get_comment_body(trimmed: &str, prefix: &str) -> String {
    trimmed.strip_prefix(prefix)
        .unwrap_or(trimmed)
        .trim()
        .to_lowercase()
}

/// Check if comment has a prefix we should keep (TODO:, FIXME:, etc.)
fn has_keep_prefix(comment_body: &str) -> bool {
    KEEP_PREFIXES.iter().any(|p| comment_body.starts_with(p))
}

/// Check if comment contains a "why" keyword
fn has_why_keyword(comment_body: &str) -> bool {
    WHY_KEYWORDS.iter().any(|kw| comment_body.contains(kw))
}

/// Check if comment contains version numbers or issue references
fn has_version_or_reference(comment_body: &str) -> bool {
    // Issue references: #123, GH-123, ISSUE-123
    let re = Regex::new(r"(#\d+|GH-\d+|ISSUE-\d+|v?\d+\.\d+\.\d+)").unwrap();
    re.is_match(comment_body)
}

/// Check if a line looks like it might be inside a string literal
fn looks_like_string_content(line: &str, _prefix: &str) -> bool {
    // If the line has content before the comment prefix that looks like a string
    // e.g.: println!("Hello // not a comment")
    // This is a heuristic check
    let before_comment = if let Some(idx) = line.find("//") {
        &line[..idx]
    } else if let Some(idx) = line.find('#') {
        &line[..idx]
    } else {
        return false;
    };
    
    // Count unescaped quotes before the comment
    let mut quote_count = 0;
    let mut chars = before_comment.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            chars.next(); // Skip escaped char
            continue;
        }
        if c == '"' || c == '\'' {
            quote_count += 1;
        }
    }
    
    // Odd number of quotes = likely inside a string
    quote_count % 2 == 1
}

/// Check if this is a doc comment (/// or //! in Rust, """ in Python)
fn is_doc_comment(trimmed: &str, prefix: &str) -> bool {
    match prefix {
        "//" => trimmed.starts_with("///") || trimmed.starts_with("//!"),
        "#" => false, // Python doesn't have doc comments in the same way
        _ => false,
    }
}

/// Check if a line is a decorative separator (// ====, // ----, etc.)
fn is_separator_line(trimmed: &str, prefix: &str) -> bool {
    if !trimmed.starts_with(prefix) {
        return false;
    }
    let body = trimmed.strip_prefix(prefix).unwrap_or("").trim();
    // Separator: all same character and length > 3
    if body.len() < 3 {
        return false;
    }
    let first_char = body.chars().next().unwrap();
    matches!(first_char, '=' | '-' | '*' | '~' | '#') 
        && body.chars().all(|c| c == first_char)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_simple_doc_comment() {
        let input = "/// User's display name\npub name: String,";
        let result = filter_message(&format!("```rs\n{}\n```", input));
        // Should strip the doc comment but keep the code
        assert!(!result.filtered_content.contains("User's display name"));
        assert!(result.filtered_content.contains("pub name: String"));
    }

    #[test]
    fn test_keep_why_comment() {
        let input = "// Using RwLock because read-heavy workload\npub users: RwLock<HashMap>,";
        let result = filter_message(&format!("```rs\n{}\n```", input));
        assert!(result.filtered_content.contains("because"));
        assert!(result.filtered_content.contains("RwLock"));
    }

    #[test]
    fn test_keep_todo() {
        let input = "// TODO: fix race condition here\nfn process() {}";
        let result = filter_message(&format!("```rs\n{}\n```", input));
        assert!(result.filtered_content.contains("TODO"));
    }

    #[test]
    fn test_no_strip_outside_code_block() {
        let input = "Here is some text with # a comment\nAnd more text";
        let result = filter_message(input);
        assert_eq!(result.filtered_content, format!("{}\n", input).trim_end_matches('\n'));
    }

    #[test]
    fn test_keep_url() {
        let input = "// See https://example.com/docs for details\nlet x = 1;";
        let result = filter_message(&format!("```rs\n{}\n```", input));
        assert!(result.filtered_content.contains("https://"));
    }

    #[test]
    fn test_strip_separator() {
        let input = "// ============================\nfn main() {}\n// ============================";
        let result = filter_message(&format!("```rs\n{}\n```", input));
        assert!(!result.filtered_content.contains("===="));
        assert!(result.filtered_content.contains("fn main()"));
    }

    #[test]
    fn test_collapse_blank_lines() {
        let input = "fn a() {}\n\n\n\nfn b() {}";
        let result = filter_message(&format!("```rs\n{}\n```", input));
        assert!(!result.filtered_content.contains("\n\n\n"));
    }

    #[test]
    fn test_python_comment() {
        let input = "# This is a simple comment\ndef hello():\n    pass";
        let result = filter_message(&format!("```py\n{}\n```", input));
        assert!(!result.filtered_content.contains("This is a simple comment"));
        assert!(result.filtered_content.contains("def hello()"));
    }

    #[test]
    fn test_keep_long_comment() {
        let long_comment = format!("// This is a very long explanation about why we need to use this particular algorithm instead of the standard approach because the standard approach has a known issue with memory allocation patterns that causes fragmentation in long-running processes. See issue #1234 for details.");
        let input = format!("{}\nlet x = 1;", long_comment);
        let result = filter_message(&format!("```rs\n{}\n```", input));
        assert!(result.filtered_content.contains("algorithm"));
    }

    #[test]
    fn test_unknown_language_no_strip() {
        let input = "# This is yaml\nkey: value";
        let result = filter_message(&format!("```yaml\n{}\n```", input));
        assert!(result.filtered_content.contains("This is yaml"));
    }
}
