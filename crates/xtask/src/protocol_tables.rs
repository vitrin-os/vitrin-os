// SPDX-License-Identifier: Apache-2.0
//! `cargo xtask protocol-tables --check` -- the IDL-derived prose gate.
//!
//! # The defect this exists to make impossible
//!
//! [`crate::verb_sets`] holds the enumerations of **verbs**. This module holds
//! the two enumerations of the IDL's own **structure** that prose restates, and
//! it exists because a fourth review of issue #196 found the same shape a
//! fourth time -- an item appended to a closed normative enumeration without
//! the enumeration moving:
//!
//! * `docs/protocol/04-vitrin_grant.md`'s header read *"3 requests"* after
//!   `get_egress` made it four. All fifteen interface page headers were
//!   enumerated by hand against the IDL to find that one; nothing did it
//!   mechanically, and nothing had.
//! * `docs/protocol/00-conventions.md` §2.3 opens *"Every `string` argument
//!   documents a maximum byte length"* and then listed **fifteen** of the
//!   **seventeen** the IDL declares -- both of the egress facet's `host`
//!   arguments were missing, and the literal `253` appeared nowhere on the
//!   page while the IDL gives a paragraph to why 253.
//!
//! Both values are **structurally derivable**: an interface's version, its
//! request count and its event count are attributes and child counts, and every
//! `string` argument's bound is already resolved into the IR
//! (`ArgType::String { max_bytes }`, parsed from the `(max N bytes)` token,
//! which `vitrin-scanner` refuses to default). So neither is a human judgement
//! and neither should be stated from memory.
//!
//! # What this deliberately does NOT do, and why
//!
//! **It does not generate either surface.** The header line carries a
//! connection class and a `@verb` that are prose choices, and §2.3's third
//! column carries the *reasoning* for each bound -- "the SPIFFE-ID maximum",
//! "measured, and 4039 bytes clear of the frame ceiling" -- which no tool can
//! produce and which is most of the value of the table. What is held is the
//! **set and the numbers**, which is the only part that was ever wrong.
//!
//! **It does not hold `docs/protocol/00-conventions.md` §6's three delivery
//! class counts** (six reply-bearing, seven structural mints, twelve
//! fire-and-forget). That section says why in its own words and the reason
//! survives this module: which class a request belongs to is a judgement about
//! its delivery contract, not something the IDL states, so a tool could check
//! the total against `<request name=` and could not check the split. A gate
//! that checked only the total would report green on a request moved from one
//! class to another, which is the drift that actually happened there.
//!
//! **It does not hold the header's `Since:` field or its `(all since 2)`
//! annotation beyond the one case it can.** `since` is per-message, and
//! "this interface is new at protocol version 2" is a claim about the
//! *document*, not an attribute. The one derivable half is checked: a header
//! that says its requests are *all* `since 2` must be describing an interface
//! whose every request carries `since="2"`.
//!
//! # The registry is the IDL, not a list
//!
//! There is no hand-kept table of pages here, on purpose: `docs/protocol/`
//! names its pages `NN-<interface>.md`, so the pairing is already written down
//! in the file names. The check is a **bijection** -- every interface in the
//! IDL has exactly one page, every page names an interface that exists -- which
//! is what makes a new interface with no prose page red without anyone
//! remembering to register it. `00-conventions.md` is the one page that names
//! no interface and is excluded by name.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};

use vitrin_scanner::ir::{ArgType, Protocol};

/// The IDL, relative to the workspace root.
const XML_PATH: &str = "protocol/vitrin-v0.xml";
/// The directory holding one prose page per interface.
const PROSE_DIR: &str = "docs/protocol";
/// The one page in [`PROSE_DIR`] that is not an interface page.
const CONVENTIONS: &str = "00-conventions.md";
/// The heading that opens the per-argument string-bound registry.
const BOUNDS_HEADING: &str = "### 2.3 Per-argument string bounds";
/// The literal each interface page's header line opens with.
const VERSION_FIELD: &str = "**Interface version:**";
/// The literal introducing the message counts on that same line.
const MESSAGES_FIELD: &str = "**Messages:**";
/// The separator between the header line's fields.
const FIELD_SEP: char = '·';

// ---------------------------------------------------------------------------
// The interface page headers
// ---------------------------------------------------------------------------

/// What one page's header line claims about its interface.
#[derive(Debug, PartialEq, Eq)]
struct Header {
    line_no: usize,
    version: u32,
    requests: u32,
    /// `None` when the header states no event clause at all, which is legal
    /// only for an interface that has no events.
    events: Option<u32>,
    /// Whether the requests clause claims they are *all* `since 2`.
    requests_all_since_2: bool,
}

/// Parse the header line of one interface page.
///
/// Deliberately strict about **plurality**: `1 request` and `2 requests` are
/// each the only accepted spelling of their number. A parser that accepted
/// either would let `1 requests` through, and a page that had been edited from
/// a plural to a singular without its number moving is exactly the drift here.
fn parse_header(text: &str) -> Result<Header> {
    let (line_no, line) = text
        .lines()
        .enumerate()
        .find(|(_, l)| l.trim_start().starts_with(VERSION_FIELD))
        .map(|(i, l)| (i + 1, l))
        .ok_or_else(|| {
            anyhow!("no header line beginning `{VERSION_FIELD}` (every interface page has one)")
        })?;

    let version =
        leading_number(after(line, VERSION_FIELD).ok_or_else(|| {
            anyhow!("line {line_no}: `{VERSION_FIELD}` is not followed by anything")
        })?)
        .map(|(n, _)| n)
        .ok_or_else(|| anyhow!("line {line_no}: `{VERSION_FIELD}` is not followed by a number"))?;

    let messages = after(line, MESSAGES_FIELD)
        .ok_or_else(|| anyhow!("line {line_no}: the header line carries no `{MESSAGES_FIELD}`"))?;
    // The field runs to the next `·` or to the end of the line, so a `@verb`
    // field after it is never mistaken for part of the counts.
    let field = messages.split(FIELD_SEP).next().unwrap_or(messages).trim();

    let (requests, rest) = leading_number(field).ok_or_else(|| {
        anyhow!("line {line_no}: `{MESSAGES_FIELD}` is not followed by a request count")
    })?;
    let rest = expect_noun(rest, requests, "request", line_no)?;

    // `expect_noun`'s return is discarded on purpose: nothing follows the event
    // clause that this parser reads, and it is called for its refusal.
    let events = match rest.split_once('+') {
        Some((_, after_plus)) => {
            let (n, rest) = leading_number(after_plus).ok_or_else(|| {
                anyhow!("line {line_no}: the `+` in the messages field is not followed by a count")
            })?;
            expect_noun(rest, n, "event", line_no)?;
            Some(n)
        }
        None => None,
    };

    Ok(Header {
        line_no,
        version,
        requests,
        events,
        requests_all_since_2: normalize(field).contains("all since 2"),
    })
}

/// The text after the first occurrence of `needle`, or `None`.
fn after<'a>(hay: &'a str, needle: &str) -> Option<&'a str> {
    hay.split_once(needle).map(|(_, rest)| rest)
}

/// The leading ASCII-decimal number of `s` once leading non-digits that are not
/// word characters have been skipped, plus everything after it.
///
/// Skipping is limited to whitespace and Markdown emphasis so that a stray word
/// between the field name and its number is an error rather than something the
/// parser reads past.
fn leading_number(s: &str) -> Option<(u32, &str)> {
    let s = s.trim_start_matches([' ', '\t', '*', '_']);
    // `find` returns `None` when every byte is a digit, which is the whole
    // cell in `| ... | 32 |`; that is the end of the run, not the absence of
    // one.
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if end == 0 {
        return None;
    }
    s[..end].parse().ok().map(|n| (n, &s[end..]))
}

/// Consume the noun that must follow a count, with the plurality that count
/// requires. Returns the text after it.
fn expect_noun<'a>(s: &'a str, count: u32, noun: &str, line_no: usize) -> Result<&'a str> {
    let want = if count == 1 {
        noun.to_string()
    } else {
        format!("{noun}s")
    };
    let rest = s.trim_start();
    rest.strip_prefix(&want)
        .filter(|after| !after.starts_with(|c: char| c.is_ascii_alphabetic()))
        .ok_or_else(|| {
            anyhow!(
                "line {line_no}: `{count}` must be followed by `{want}` (found `{}`)",
                rest.chars().take(24).collect::<String>()
            )
        })
}

/// Collapse whitespace and Markdown emphasis so a phrase check is not defeated
/// by a line wrap or a `*(...)*`.
fn normalize(s: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if matches!(c, '*' | '_' | '(' | ')' | '`') {
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

// ---------------------------------------------------------------------------
// The per-argument string bounds (00-conventions.md, section 2.3)
// ---------------------------------------------------------------------------

/// One row of the string-bound registry: what it keys on and what it claims.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Bound {
    /// `interface.message`, exactly as the table's first column spells it.
    message: String,
    arg: String,
    max: u32,
}

impl Bound {
    fn render(&self) -> String {
        format!("{}.{} = {}", self.message, self.arg, self.max)
    }
}

/// Every `string` argument in the IDL, in document order.
fn derive_bounds(protocol: &Protocol) -> Vec<Bound> {
    let mut out = Vec::new();
    for iface in &protocol.interfaces {
        for msg in iface.requests.iter().chain(iface.events.iter()) {
            for arg in &msg.args {
                if let ArgType::String { max_bytes } = arg.ty {
                    out.push(Bound {
                        message: format!("{}.{}", iface.name, msg.name),
                        arg: arg.name.clone(),
                        max: max_bytes,
                    });
                }
            }
        }
    }
    out
}

/// Parse the rows of §2.3's table out of `00-conventions.md`.
///
/// Scoped to the section: it starts at [`BOUNDS_HEADING`] and stops at the next
/// heading of any level, so no other table on the page can contribute a row and
/// a section that has been renamed away is an error rather than an empty
/// (and therefore silently passing) table.
fn parse_bounds_table(text: &str) -> Result<Vec<Bound>> {
    let body = text
        .split_once(BOUNDS_HEADING)
        .map(|(_, rest)| rest)
        .ok_or_else(|| anyhow!("{PROSE_DIR}/{CONVENTIONS} no longer has a `{BOUNDS_HEADING}`"))?;
    let body = body
        .split("\n#")
        .next()
        .ok_or_else(|| anyhow!("`{BOUNDS_HEADING}` is unterminated"))?;

    let mut out = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line
            .trim_matches('|')
            .split('|')
            .map(|c| c.trim())
            .collect();
        if cells.len() != 3 {
            bail!(
                "the `{BOUNDS_HEADING}` table has a row with {} cell(s), not 3: `{line}`",
                cells.len()
            );
        }
        // The header row and the `|---|` separator.
        if cells[0] == "Interface.message" || cells[0].starts_with("---") {
            continue;
        }
        let message = cells[0].trim_matches('`');
        let arg = cells[1].trim_matches('`');
        let max = leading_number(cells[2]).map(|(n, _)| n).ok_or_else(|| {
            anyhow!("the `{message}.{arg}` row's third cell does not begin with a number")
        })?;
        out.push(Bound {
            message: message.to_string(),
            arg: arg.to_string(),
            max,
        });
    }
    if out.is_empty() {
        bail!("the `{BOUNDS_HEADING}` table parsed to zero rows, which cannot be right");
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// The check
// ---------------------------------------------------------------------------

/// Run the whole check against a workspace root. Reads only; writes nothing.
pub fn check(root: &Path) -> Result<String> {
    let xml_path = root.join(XML_PATH);
    let xml = std::fs::read_to_string(&xml_path)
        .with_context(|| format!("reading {}", xml_path.display()))?;
    let protocol = vitrin_scanner::parse::parse(&xml)
        .with_context(|| format!("parsing {}", xml_path.display()))?;

    let mut failures: Vec<String> = Vec::new();

    // -- the bijection between interfaces and prose pages -------------------
    let mut pages: BTreeMap<String, String> = BTreeMap::new();
    let dir = root.join(PROSE_DIR);
    for entry in std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".md") || name == CONVENTIONS {
            continue;
        }
        let stem = &name[..name.len() - 3];
        match stem.split_once('-') {
            Some((digits, iface))
                if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) =>
            {
                if let Some(previous) = pages.insert(iface.to_string(), name.clone()) {
                    failures.push(format!(
                        "{PROSE_DIR}/: two pages claim the interface `{iface}` -- `{previous}` \
                         and `{name}`. One page per interface is what makes the header check a \
                         bijection."
                    ));
                }
            }
            _ => failures.push(format!(
                "{PROSE_DIR}/{name}: not named `NN-<interface>.md`. Every page in this \
                 directory is either `{CONVENTIONS}` or one interface's page, and the file \
                 name is how this gate pairs the two."
            )),
        }
    }

    let declared: BTreeSet<&str> = protocol
        .interfaces
        .iter()
        .map(|i| i.name.as_str())
        .collect();
    for iface in pages.keys() {
        if !declared.contains(iface.as_str()) {
            failures.push(format!(
                "{PROSE_DIR}/{}: names `{iface}`, which {XML_PATH} does not declare.",
                pages[iface]
            ));
        }
    }

    // -- each page's header against its interface ---------------------------
    let mut checked = 0usize;
    for iface in &protocol.interfaces {
        let Some(file) = pages.get(&iface.name) else {
            failures.push(format!(
                "{XML_PATH} declares `{}` and {PROSE_DIR}/ has no `NN-{}.md`. The protocol \
                 authoring rule (CLAUDE.md) pairs every interface with a prose page.",
                iface.name, iface.name
            ));
            continue;
        };
        let path = dir.join(file);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let header = match parse_header(&text) {
            Ok(h) => h,
            Err(e) => {
                failures.push(format!("{PROSE_DIR}/{file}: {e}"));
                continue;
            }
        };
        checked += 1;

        let requests = iface.requests.len() as u32;
        let events = iface.events.len() as u32;
        if header.version != iface.version {
            failures.push(format!(
                "{PROSE_DIR}/{file}:{}: the header says interface version {}; {XML_PATH} says {}.",
                header.line_no, header.version, iface.version
            ));
        }
        if header.requests != requests {
            failures.push(format!(
                "{PROSE_DIR}/{file}:{}: the header says {} request(s); `{}` has {requests}.",
                header.line_no, header.requests, iface.name
            ));
        }
        match header.events {
            Some(stated) if stated != events => failures.push(format!(
                "{PROSE_DIR}/{file}:{}: the header says {stated} event(s); `{}` has {events}.",
                header.line_no, iface.name
            )),
            // A header may omit the event clause entirely, and two pages do --
            // but only an interface with no events may omit it, or the omission
            // is a count silently dropped rather than a count that is zero.
            None if events != 0 => failures.push(format!(
                "{PROSE_DIR}/{file}:{}: the header states no event count, which is legal only \
                 for an interface with no events; `{}` has {events}.",
                header.line_no, iface.name
            )),
            _ => {}
        }
        if header.requests_all_since_2 && !iface.requests.iter().all(|r| r.since == 2) {
            failures.push(format!(
                "{PROSE_DIR}/{file}:{}: the header says the requests are all `since 2`; \
                 `{}` has one or more that are not.",
                header.line_no, iface.name
            ));
        }
    }

    // -- section 2.3 against every `string` argument ------------------------
    let conventions = dir.join(CONVENTIONS);
    let conventions_text = std::fs::read_to_string(&conventions)
        .with_context(|| format!("reading {}", conventions.display()))?;
    let derived = derive_bounds(&protocol);
    match parse_bounds_table(&conventions_text) {
        Ok(stated) => {
            let want: BTreeSet<Bound> = derived.iter().cloned().collect();
            let have: BTreeSet<Bound> = stated.iter().cloned().collect();
            if stated.len() != have.len() {
                failures.push(format!(
                    "{PROSE_DIR}/{CONVENTIONS} {BOUNDS_HEADING}: {} row(s) but only {} distinct \
                     ones -- a duplicated row makes the registry's size a lie.",
                    stated.len(),
                    have.len()
                ));
            }
            for missing in want.difference(&have) {
                failures.push(format!(
                    "{PROSE_DIR}/{CONVENTIONS} {BOUNDS_HEADING}: {XML_PATH} declares \
                     `{}` and the table has no row for it. The section opens with the word \
                     `Every`.",
                    missing.render()
                ));
            }
            for extra in have.difference(&want) {
                failures.push(format!(
                    "{PROSE_DIR}/{CONVENTIONS} {BOUNDS_HEADING}: the table states `{}`, which \
                     {XML_PATH} does not (wrong bound, wrong argument name, or a row left \
                     behind by a deleted message).",
                    extra.render()
                ));
            }
        }
        Err(e) => failures.push(format!("{PROSE_DIR}/{CONVENTIONS}: {e}")),
    }

    if !failures.is_empty() {
        bail!(
            "protocol-table drift ({} problem(s)):\n  - {}",
            failures.len(),
            failures.join("\n  - ")
        );
    }

    let mut report = format!(
        "protocol-tables: {checked} interface page header(s) and {} string-bound row(s) agree \
         with {XML_PATH}.\n",
        derived.len()
    );
    for iface in &protocol.interfaces {
        let _ = writeln!(
            report,
            "  {:24} v{} {} request(s) + {} event(s)",
            iface.name,
            iface.version,
            iface.requests.len(),
            iface.events.len()
        );
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root")
    }

    fn protocol() -> Protocol {
        let xml = std::fs::read_to_string(root().join(XML_PATH)).expect("the IDL");
        vitrin_scanner::parse::parse(&xml).expect("the IDL parses")
    }

    /// The gate itself. Lives as a test as well as a subcommand so that
    /// `cargo test --workspace` -- which CI already runs -- fails on drift
    /// even before anyone wires the subcommand into a job.
    #[test]
    fn every_interface_page_and_the_bounds_table_agree_with_the_idl() {
        match check(&root()) {
            Ok(report) => eprintln!("{report}"),
            Err(e) => panic!("{e:?}"),
        }
    }

    /// Non-vacuity for the header half, at the exact value this branch got
    /// wrong: `vitrin_grant` gained a fourth request and its page said three.
    /// It then gained a FIFTH -- `get_powerbox` and `get_egress` landed from
    /// two branches at once -- so the page's real count is read from the IDL
    /// rather than written here, and only the undercount is a literal.
    #[test]
    fn a_header_that_undercounts_its_requests_is_a_failure() {
        let page = "# vitrin_grant\n\n**Interface version:** 2 · **Connection class:** \
                    principal · **Messages:** 3 requests *(all since 2)* + 2 events\n";
        let header = parse_header(page).expect("parses");
        assert_eq!(header.version, 2);
        assert_eq!(header.requests, 3);
        assert_eq!(header.events, Some(2));
        assert!(header.requests_all_since_2);

        let grant = protocol()
            .interface("vitrin_grant")
            .expect("vitrin_grant")
            .clone();
        assert!(
            grant.requests.len() > header.requests as usize,
            "the IDL is what makes the header above wrong: it declares {} request(s)",
            grant.requests.len()
        );
        assert_ne!(header.requests, grant.requests.len() as u32);
    }

    /// The event clause may be omitted, and both spellings of zero parse to a
    /// value this check can tell apart from a dropped count.
    #[test]
    fn an_omitted_event_clause_is_distinguishable_from_a_stated_zero() {
        let omitted = parse_header(
            "**Interface version:** 1 · **Messages:** 1 request · **`@verb`:** `layout_focus`\n",
        )
        .expect("parses");
        assert_eq!(omitted.requests, 1);
        assert_eq!(omitted.events, None);

        let stated =
            parse_header("**Interface version:** 1 · **Messages:** 1 request + 0 events\n")
                .expect("parses");
        assert_eq!(stated.events, Some(0));
    }

    /// The `@verb` field sits after the counts on the same line, separated by
    /// the same `·`. A parser that read to end-of-line would find its numbers
    /// in the wrong place the moment a verb name contained one.
    #[test]
    fn a_trailing_field_is_not_read_as_part_of_the_counts() {
        let h = parse_header(
            "**Interface version:** 1 · **Since:** protocol version 2 · **Connection class:** \
             principal · **Messages:** 1 request + 2 events · **`@verb`:** `egress`\n",
        )
        .expect("parses");
        assert_eq!((h.version, h.requests, h.events), (1, 1, Some(2)));
        assert!(!h.requests_all_since_2);
    }

    /// Plurality is part of the claim, not decoration: a count edited without
    /// its noun is the same drift wearing a smaller hat.
    #[test]
    fn plurality_must_match_the_count() {
        assert!(
            parse_header("**Interface version:** 1 · **Messages:** 2 request + 1 event\n").is_err()
        );
        assert!(
            parse_header("**Interface version:** 1 · **Messages:** 1 requests + 1 event\n")
                .is_err()
        );
        assert!(
            parse_header("**Interface version:** 1 · **Messages:** 1 request + 2 event\n").is_err()
        );
        assert!(
            parse_header("**Interface version:** 1 · **Messages:** 1 request + 2 events\n").is_ok()
        );
    }

    /// A missing header, or one with no counts at all, is an error rather than
    /// a page that quietly checks nothing.
    #[test]
    fn a_page_with_no_header_is_a_failure() {
        assert!(parse_header("# vitrin_thing\n\nprose only\n").is_err());
        assert!(
            parse_header("**Interface version:** 1 · **Connection class:** principal\n").is_err()
        );
        assert!(parse_header("**Interface version:** · **Messages:** 1 request\n").is_err());
    }

    /// Non-vacuity for the bounds half: the derived set is the full nineteen
    /// and contains both of the arguments §2.3 had missed.
    ///
    /// Seventeen when this test was written; the powerbox's two `name`
    /// arguments merged in from a parallel branch and made it nineteen.
    #[test]
    fn the_bounds_derivation_finds_every_string_argument() {
        let bounds = derive_bounds(&protocol());
        assert_eq!(bounds.len(), 19);
        assert!(bounds.contains(&Bound {
            message: "vitrin_egress.request_connect".into(),
            arg: "host".into(),
            max: 253,
        }));
        assert!(bounds.contains(&Bound {
            message: "vitrin_egress.connected".into(),
            arg: "host".into(),
            max: 253,
        }));
        // The bound is the IDL's, not a default: `vitrin-scanner` refuses a
        // `string` arg whose summary carries no `(max N bytes)` token, so a
        // zero here would mean the parser, not the document, had changed.
        assert!(bounds.iter().all(|b| b.max > 0));
    }

    /// The table parser reads the section it is aimed at and nothing else --
    /// the page carries a dozen other three-column tables.
    #[test]
    fn the_bounds_parser_reads_only_its_own_section() {
        let text = format!(
            "### 2.2 Something\n\n| a | b | c |\n|---|---|---|\n| `x.y` | `z` | 9 |\n\n\
             {BOUNDS_HEADING}\n\n| Interface.message | Argument | Max bytes |\n|---|---|---|\n\
             | `vitrin_handshake.hello` | `identity` | 2048 (a reason) |\n\n\
             ### 2.4 After\n\n| a | b | c |\n|---|---|---|\n| `p.q` | `r` | 7 |\n"
        );
        let rows = parse_bounds_table(&text).expect("parses");
        assert_eq!(
            rows,
            vec![Bound {
                message: "vitrin_handshake.hello".into(),
                arg: "identity".into(),
                max: 2048,
            }]
        );
        // A page that has lost the section is an error, never zero rows that
        // agree with nothing.
        assert!(parse_bounds_table("### 9 Elsewhere\n").is_err());
        assert!(parse_bounds_table(&format!("{BOUNDS_HEADING}\n\nprose\n")).is_err());
    }

    /// A row whose bound disagrees with the IDL is caught by value, not only a
    /// row that is absent. Both directions are the same defect.
    #[test]
    fn a_wrong_bound_is_not_the_same_row() {
        let right = Bound {
            message: "vitrin_egress.connected".into(),
            arg: "host".into(),
            max: 253,
        };
        let wrong = Bound {
            max: 255,
            ..right.clone()
        };
        assert_ne!(right, wrong);
        assert!(derive_bounds(&protocol()).contains(&right));
        assert!(!derive_bounds(&protocol()).contains(&wrong));
    }

    /// The registry is the file names, so an interface with no page is red
    /// without anyone remembering to add a row anywhere.
    #[test]
    fn every_declared_interface_has_exactly_one_page() {
        let dir = root().join(PROSE_DIR);
        let mut pages: Vec<String> = std::fs::read_dir(&dir)
            .expect("docs/protocol")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".md") && n != CONVENTIONS)
            .collect();
        pages.sort();
        let protocol = protocol();
        assert_eq!(pages.len(), protocol.interfaces.len());
        for iface in &protocol.interfaces {
            assert_eq!(
                pages
                    .iter()
                    .filter(|p| p.ends_with(&format!("-{}.md", iface.name)))
                    .count(),
                1,
                "`{}` must have exactly one page in {PROSE_DIR}/",
                iface.name
            );
        }
    }
}
