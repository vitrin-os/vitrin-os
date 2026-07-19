//! The strict TOML subset the core's configuration files are written in --
//! one lexer, shared by every schema.
//!
//! Plan risk R7 names configuration parsing as the classic TCB
//! dependency-creep vector, so the core parses its own configuration rather
//! than linking a TOML crate (and emphatically not a derive-macro
//! serialization framework). The corollary of hand-rolling is that there
//! must be exactly **one** hand-rolled lexer: two would be two places for
//! hostile config bytes to be mis-scanned and two dialects for an operator
//! to learn. This module is that one place. The schema modules --
//! [`crate::identity`] (`principals.toml`, P1.4.1) and [`crate::realm`]
//! (`realm.toml`, P1.5.1) -- own only their key vocabulary, their
//! required-key and cross-key rules, and their own error taxonomies.
//!
//! # The subset
//!
//! Accepted, and nothing else:
//!
//! - comments (`# ...`) and blank lines;
//! - array-of-table headers `[[name]]`, `name` in `[A-Za-z0-9_]+`;
//! - `key = "basic string"`, with only the `\\` and `\"` escapes;
//! - `key = <non-negative decimal integer>` fitting `u32`;
//! - `key = ["a", "b"]` -- a single-line array of basic strings, trailing
//!   comma allowed.
//!
//! Everything else -- single-bracket tables, dotted keys, inline tables,
//! nested or multi-line arrays, multi-line and literal strings, other
//! escapes, floats, booleans, datetimes -- is an error, never a guess.
//! Every file this subset accepts is valid TOML, so external tooling
//! interoperates and a later swap to a full parser changes nothing an
//! operator can see.
//!
//! # Why refusal rather than tolerance
//!
//! A config key the core does not understand is either a typo or a
//! misunderstanding of what the core enforces. Both are fail-open traps
//! when ignored: `env_alow = [...]` that silently does nothing reads, to
//! the operator, exactly like an allowlist that works. Refusing at load --
//! with the line number and the offending text -- is the only outcome that
//! cannot be mistaken for success.

use std::fmt;

/// A lexical or grammatical problem at one line of a config file. The
/// schema modules wrap this into their own error taxonomy (which is what
/// attaches the file path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubsetError {
    /// 1-based line number, as an operator's editor counts.
    pub line: usize,
    pub detail: String,
}

impl SubsetError {
    fn at(line: usize, detail: &str) -> Self {
        Self {
            line,
            detail: detail.to_owned(),
        }
    }
}

impl fmt::Display for SubsetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.detail)
    }
}

/// Split an array-of-table header (`[[name]]`) off the front of `line`.
/// Returns the header text (brackets included) and the remainder, or
/// `None` when the line does not open one -- which includes TOML's
/// single-bracket `[name]` form, deliberately outside the subset: every
/// schema here is a list of tables, and accepting both spellings would
/// make "one table" and "one table in a list" indistinguishable on sight.
pub(crate) fn table_header(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix("[[")?;
    let end = rest.find("]]")?;
    let name = &rest[..end];
    if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !name.is_empty() {
        Some((&line[..end + 4], &rest[end + 2..]))
    } else {
        None
    }
}

/// Parse a TOML basic string (`"..."`) that spans the whole value: the
/// remainder after the closing quote may hold only whitespace or a
/// comment.
pub(crate) fn basic_string(value: &str, line: usize) -> Result<String, SubsetError> {
    let (parsed, rest) = scan_basic_string(value, line)?;
    trailing_blank(rest, line, "string value")?;
    Ok(parsed)
}

/// Parse a single-line array of basic strings (`["a", "b"]`). A trailing
/// comma before `]` is accepted (TOML 1.0 allows it); a leading or double
/// comma is not. Nested arrays, multi-line arrays, and non-string elements
/// are outside the subset.
pub(crate) fn string_array(value: &str, line: usize) -> Result<Vec<String>, SubsetError> {
    let mut rest = value
        .strip_prefix('[')
        .ok_or_else(|| SubsetError::at(line, "expected an array value, e.g. `[\"a\", \"b\"]`"))?
        .trim_start();
    let mut out: Vec<String> = Vec::new();
    loop {
        if let Some(after) = rest.strip_prefix(']') {
            trailing_blank(after, line, "array value")?;
            return Ok(out);
        }
        if !out.is_empty() {
            // A separator is required between elements; after it a `]`
            // closes the array (the accepted trailing comma).
            rest = rest
                .strip_prefix(',')
                .ok_or_else(|| SubsetError::at(line, "expected `,` or `]` in array value"))?
                .trim_start();
            if let Some(after) = rest.strip_prefix(']') {
                trailing_blank(after, line, "array value")?;
                return Ok(out);
            }
        }
        // Each turn consumes at least the two quotes of one element or
        // errors, so this loop always terminates.
        let (element, after) = scan_basic_string(rest, line)?;
        out.push(element);
        rest = after.trim_start();
    }
}

/// Parse a bare non-negative decimal integer fitting `u32`; the remainder
/// may hold only whitespace or a comment. Leading zeros are rejected:
/// TOML 1.0 forbids them, and the subset's invariant is that every
/// accepted file is valid TOML -- `007` must not load today only to break
/// under external tooling or a future parser swap. A bare `0` stays legal.
pub(crate) fn integer(value: &str, line: usize) -> Result<u32, SubsetError> {
    let end = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    let (digits, rest) = value.split_at(end);
    if digits.is_empty() {
        return Err(SubsetError::at(
            line,
            "expected a non-negative integer value",
        ));
    }
    if digits.len() > 1 && digits.starts_with('0') {
        return Err(SubsetError::at(
            line,
            "leading zeros are not valid TOML integers",
        ));
    }
    trailing_blank(rest, line, "integer value")?;
    digits
        .parse::<u32>()
        .map_err(|_| SubsetError::at(line, "integer does not fit u32"))
}

/// Scan one basic string off the front of `value`, returning it and the
/// unconsumed remainder. The shared primitive behind [`basic_string`] and
/// [`string_array`], so a string means the same thing in both positions.
fn scan_basic_string(value: &str, line: usize) -> Result<(String, &str), SubsetError> {
    let mut chars = value.char_indices();
    match chars.next() {
        Some((_, '"')) => {}
        _ => {
            return Err(SubsetError::at(
                line,
                "expected a double-quoted string value",
            ))
        }
    }
    let mut out = String::new();
    let mut escaped = false;
    for (i, c) in chars {
        if escaped {
            match c {
                '\\' | '"' => out.push(c),
                _ => {
                    return Err(SubsetError::at(
                        line,
                        "escape outside the supported subset (only \\\\ and \\\" are allowed)",
                    ))
                }
            }
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '"' => return Ok((out, &value[i + 1..])),
            // Rejecting control characters here is also what keeps an
            // interior NUL out of every value the core later hands to
            // exec/setenv, where a C string would silently truncate.
            c if c.is_control() => {
                return Err(SubsetError::at(
                    line,
                    "control character inside string value",
                ))
            }
            c => out.push(c),
        }
    }
    Err(SubsetError::at(line, "unterminated string value"))
}

/// Require that only whitespace or a comment follows a parsed value.
fn trailing_blank(rest: &str, line: usize, what: &str) -> Result<(), SubsetError> {
    if matches!(rest.trim_start().chars().next(), None | Some('#')) {
        Ok(())
    } else {
        Err(SubsetError::at(
            line,
            &format!("trailing content after {what}"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_strings_accept_the_two_escapes_and_reject_the_rest() {
        assert_eq!(basic_string(r#""plain""#, 1).unwrap(), "plain");
        assert_eq!(
            basic_string(r#""with#hash-and-\\-and-\"-inside""#, 1).unwrap(),
            r#"with#hash-and-\-and-"-inside"#
        );
        assert_eq!(basic_string(r#""v" # trailing comment"#, 1).unwrap(), "v");
        for (value, why) in [
            (r#""a\nb""#, "escape outside the subset"),
            (r#""unterminated"#, "unterminated"),
            ("'literal'", "literal string"),
            (r#""""multi""""#, "multi-line string"),
            (r#""a" junk"#, "trailing junk"),
            ("42", "not a string at all"),
            ("[\"a\"]", "array where a string was expected"),
        ] {
            assert!(basic_string(value, 7).is_err(), "must reject: {why}");
        }
        // The line number an operator can act on rides along.
        assert_eq!(basic_string("nope", 42).unwrap_err().line, 42);
    }

    #[test]
    fn string_arrays_accept_the_documented_shapes() {
        assert_eq!(string_array("[]", 1).unwrap(), Vec::<String>::new());
        assert_eq!(string_array(r#"["a"]"#, 1).unwrap(), vec!["a"]);
        assert_eq!(
            string_array(r#"[ "a" ,  "b" , ]  # trailing comma is TOML"#, 1).unwrap(),
            vec!["a", "b"]
        );
        // An escaped quote inside an element must not end the array early.
        assert_eq!(
            string_array(r#"["a\"]", "b"]"#, 1).unwrap(),
            vec![r#"a"]"#, "b"]
        );
    }

    #[test]
    fn malformed_arrays_are_refused_and_always_terminate() {
        for (value, why) in [
            (r#"["a""#, "unterminated array"),
            (r#"["a" "b"]"#, "missing separator"),
            (r#"[, "a"]"#, "leading comma"),
            (r#"["a",, "b"]"#, "double comma"),
            (r#"[1, 2]"#, "non-string elements"),
            (r#"[["a"]]"#, "nested array"),
            (r#"["a"] junk"#, "trailing junk"),
            (r#""a""#, "string where an array was expected"),
            ("[", "bare open bracket"),
        ] {
            assert!(string_array(value, 3).is_err(), "must reject: {why}");
        }
    }

    #[test]
    fn integers_are_bare_non_negative_and_leading_zero_free() {
        assert_eq!(integer("0", 1).unwrap(), 0);
        assert_eq!(integer("1000 # comment", 1).unwrap(), 1000);
        for (value, why) in [
            ("-1", "negative"),
            ("007", "leading zero"),
            ("00", "leading zero on zero"),
            ("99999999999", "does not fit u32"),
            ("1 junk", "trailing junk"),
            ("0x10", "hexadecimal"),
            (r#""5""#, "quoted"),
        ] {
            assert!(integer(value, 5).is_err(), "must reject: {why}");
        }
    }

    #[test]
    fn only_double_bracket_headers_are_in_the_subset() {
        assert_eq!(table_header("[[realm]]"), Some(("[[realm]]", "")));
        assert_eq!(
            table_header("[[principal]] extra"),
            Some(("[[principal]]", " extra"))
        );
        // Single-bracket tables, empty and punctuated names are not headers
        // at all; the caller reports them as the parse errors they are.
        assert_eq!(table_header("[realm]"), None);
        assert_eq!(table_header("[[]]"), None);
        assert_eq!(table_header("[[a.b]]"), None);
        assert_eq!(table_header("[["), None);
    }
}
