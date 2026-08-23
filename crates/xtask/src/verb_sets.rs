// SPDX-License-Identifier: Apache-2.0
//! `cargo xtask verb-sets --check` -- the verb-enumeration drift gate.
//!
//! # The defect this exists to make impossible
//!
//! Three consecutive reviews of issue #196 found the same shape of defect and
//! nothing else: **a set of verbs, enumerated in prose, corrected in one place
//! and left stale in another.**
//!
//! * round 1: *"one more is defined and refuses `unsupported` -- `observe.cursor`"*
//!   when two were, in `docs/book/src/03-grants-consent-revocation.md`;
//! * round 2: the *facetless* verb set stated four times -- in
//!   `protocol/vitrin-v0.rng`, `protocol/test-mutations.sh`,
//!   `docs/protocol/00-conventions.md` and `docs/protocol/04-vitrin_grant.md`
//!   -- and stale in three of them. The `test-mutations.sh` copy was
//!   self-refuting: it said `observe_cursor` *"is the only one left"* inside a
//!   comment that also explains that facetless verbs gain facets over time;
//! * round 3 (this module's landing): five more, including two in
//!   `protocol/vitrin-v0.xml` itself -- the one surface `CLAUDE.md` says wins
//!   over every prose page.
//!
//! Rounds 1 and 2 fixed instances. This fixes the source: every one of those
//! sets is **derivable**, so no surface should be stating one from memory.
//!
//! # What is derived, and from where
//!
//! | set | derived from |
//! |---|---|
//! | [`SetKind::AllVerbs`] | every entry of `vitrin_grant.verb` in the IDL |
//! | [`SetKind::FacetVerbs`] | every `interface/@verb` in the IDL |
//! | [`SetKind::FacetlessVerbs`] | the first minus the second |
//! | [`SetKind::FacetInterfaces`] | every interface carrying `interface/@verb` |
//! | [`SetKind::UnservedVerbs`] | `vitrin_grant.verb` minus `SERVED_VERB_BITS`, parsed out of `crates/vitrin-core/src/grants.rs` |
//!
//! The IDL is parsed with the real scanner (`vitrin_scanner::parse`), never
//! with a private regex, so this tool and the codegen can never disagree about
//! what the document says.
//!
//! `UnservedVerbs` is the one set that is a property of the **reference core**
//! rather than of the wire, and the tool says so rather than blurring it: a
//! deployment may decline any verb it likes. It is checkable here only because
//! the two sets coincide today -- `observe_cursor` and `egress` are refused by
//! *every* deployment (no per-principal cursor delivery; and for `egress`, no
//! out-of-core mediating proxy -- the facet landed at P2.7.2 and did not make
//! the verb servable, so this parenthetical reads "no proxy" where it once
//! read "no facet at all"), which is what the spec surfaces claim, and they
//! are also exactly what this core leaves out of `SERVED_VERB_BITS`. Should a
//! verb ever be unserved *here* but servable elsewhere, the spec carriers must
//! drop the marker rather than be forced to restate a local fact.
//!
//! # The marker, and why it is not a phrase
//!
//! Each carrier declares what it enumerates with a one-line comment in its own
//! comment syntax -- invisible in every rendered form of these documents:
//!
//! ```text
//! <!-- vitrin-verb-set: facetless-verbs = observe_cursor, egress -->      (markdown, XML, RELAX NG)
//! # vitrin-verb-set: facetless-verbs = observe_cursor, egress             (shell, Python)
//! // vitrin-verb-set: facetless-verbs = observe_cursor, egress            (Rust)
//! ```
//!
//! and a carrier that also states the set's **size in words** appends the word:
//!
//! ```text
//! <!-- vitrin-verb-set: unserved-verbs = observe_cursor, egress | count: two -->
//! ```
//!
//! Three things are then checked per marker, and each closes a different half
//! of the observed defect:
//!
//! 1. **The marker's list equals the derived set, in IDL document order.** This
//!    is what goes red when a verb is appended, or gains a facet, or starts
//!    being served. It cannot be satisfied by rewording.
//! 2. **Every derived name appears in the [`PASSAGE_LINES`] lines around the
//!    marker** (marker lines stripped, so a marker can never satisfy its own
//!    check). This is a HEURISTIC and that constant's docs say exactly what it
//!    does and does not catch -- read them before relying on it. Check 1 is
//!    the one that carries the weight.
//! 3. **The stated count word is the right word for the size, and appears in
//!    the same passage.** *"it is the only one left"* has no numeral in it, so
//!    this catches only the carriers that spell a number; those are the ones
//!    that say "Six", "Two", "two more".
//!
//! Deleting a marker is red too: [`CARRIERS`] names every file, so a carrier
//! that quietly stops declaring is a failure with the path in it.
//!
//! # What this does NOT catch, stated rather than left to be discovered
//!
//! **A brand-new file that enumerates one of these sets and carries no marker
//! is invisible to this tool.** Nothing scans the tree for prose that looks
//! like an enumeration; [`CARRIERS`] is a registry a human extends. That is the
//! same residual `cargo xtask limits-check` carries for published claims, and
//! it is accepted for the same reason: closed-world discovery would have to key
//! on a verb *name*, and `egress` alone appears in some forty files that
//! enumerate nothing at all.
//!
//! What the registry does buy is that the moment a set moves, every registered
//! surface must be visited -- and the failure message names the ones that were
//! not.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

use vitrin_scanner::ir::Protocol;

use crate::build_output::BuildOutput;

/// The IDL, relative to the workspace root.
const XML_PATH: &str = "protocol/vitrin-v0.xml";
/// The RELAX NG schema for the IDL dialect, relative to the workspace root.
const RNG_PATH: &str = "protocol/vitrin-v0.rng";
/// The reference core's served-verb constant, relative to the workspace root.
const SERVED_BITS_PATH: &str = "crates/vitrin-core/src/grants.rs";
/// The name of the constant parsed out of [`SERVED_BITS_PATH`].
const SERVED_BITS_CONST: &str = "SERVED_VERB_BITS";
/// The token every marker line carries, whatever the file's comment syntax.
const MARKER: &str = "vitrin-verb-set:";

/// Which derived set a marker declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SetKind {
    /// Every entry of `vitrin_grant.verb`.
    AllVerbs,
    /// The verbs that have a facet interface (`interface/@verb`).
    FacetVerbs,
    /// The verbs that have none.
    FacetlessVerbs,
    /// The interfaces that carry `interface/@verb`.
    FacetInterfaces,
    /// The verbs this core does not serve.
    UnservedVerbs,
}

impl SetKind {
    fn wire_name(self) -> &'static str {
        match self {
            SetKind::AllVerbs => "all-verbs",
            SetKind::FacetVerbs => "facet-verbs",
            SetKind::FacetlessVerbs => "facetless-verbs",
            SetKind::FacetInterfaces => "facet-interfaces",
            SetKind::UnservedVerbs => "unserved-verbs",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        [
            SetKind::AllVerbs,
            SetKind::FacetVerbs,
            SetKind::FacetlessVerbs,
            SetKind::FacetInterfaces,
            SetKind::UnservedVerbs,
        ]
        .into_iter()
        .find(|k| k.wire_name() == s)
    }
}

impl fmt::Display for SetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.wire_name())
    }
}

/// Every file that enumerates one of these sets in its own words.
///
/// **Extend this when a new surface starts enumerating one, and delete the
/// entry when it stops.** A registered file with no marker of the kind is a
/// failure, so an entry cannot rot into decoration.
const CARRIERS: &[(&str, SetKind)] = &[
    // -- the normative surfaces (CLAUDE.md: the IDL wins over prose) --------
    (XML_PATH, SetKind::FacetlessVerbs),
    (XML_PATH, SetKind::UnservedVerbs),
    ("protocol/vitrin-v0.rng", SetKind::FacetVerbs),
    ("protocol/vitrin-v0.rng", SetKind::FacetlessVerbs),
    ("protocol/test-mutations.sh", SetKind::FacetlessVerbs),
    // -- the prose pages that restate them ---------------------------------
    ("docs/protocol/00-conventions.md", SetKind::FacetVerbs),
    ("docs/protocol/00-conventions.md", SetKind::FacetInterfaces),
    ("docs/protocol/00-conventions.md", SetKind::FacetlessVerbs),
    ("docs/protocol/00-conventions.md", SetKind::UnservedVerbs),
    ("docs/protocol/03-vitrin_realm.md", SetKind::AllVerbs),
    ("docs/protocol/04-vitrin_grant.md", SetKind::AllVerbs),
    ("docs/protocol/04-vitrin_grant.md", SetKind::FacetVerbs),
    ("docs/protocol/04-vitrin_grant.md", SetKind::FacetlessVerbs),
    ("docs/protocol/04-vitrin_grant.md", SetKind::UnservedVerbs),
    // -- the book, which is where round 1's stale copies were --------------
    ("docs/book/src/02-your-first-agent.md", SetKind::AllVerbs),
    (
        "docs/book/src/02-your-first-agent.md",
        SetKind::UnservedVerbs,
    ),
    (
        "docs/book/src/03-grants-consent-revocation.md",
        SetKind::UnservedVerbs,
    ),
    (
        "docs/book/src/06-build-your-own-client.md",
        SetKind::UnservedVerbs,
    ),
    // -- the code that implements the classification -----------------------
    (
        "crates/vitrin-protocol/tests/decode_errors.rs",
        SetKind::FacetlessVerbs,
    ),
    (SERVED_BITS_PATH, SetKind::UnservedVerbs),
    (
        "crates/vitrin-core/src/petitions.rs",
        SetKind::UnservedVerbs,
    ),
    (
        "crates/vitrin-core/src/consent/render.rs",
        SetKind::UnservedVerbs,
    ),
    // -- the SDK, which transcribes the bitfield by hand -------------------
    ("sdk/python/src/vitrin_os/protocol.py", SetKind::AllVerbs),
    ("sdk/python/src/vitrin_os/client.py", SetKind::UnservedVerbs),
    (
        "sdk/python/tests/test_verb_parity.py",
        SetKind::UnservedVerbs,
    ),
];

/// English words for the sizes a set here can plausibly take. A size with no
/// word is a hard error rather than a skipped check.
const NUMBER_WORDS: &[&str] = &[
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
    "eleven", "twelve",
];

/// One parsed marker line.
#[derive(Debug)]
struct Marker {
    kind: SetKind,
    line_no: usize,
    members: Vec<String>,
    count_word: Option<String>,
}

/// The five sets, derived once.
#[derive(Debug)]
pub struct DerivedSets {
    all_verbs: Vec<String>,
    facet_verbs: Vec<String>,
    facetless_verbs: Vec<String>,
    facet_interfaces: Vec<String>,
    unserved_verbs: Vec<String>,
}

impl DerivedSets {
    fn get(&self, kind: SetKind) -> &[String] {
        match kind {
            SetKind::AllVerbs => &self.all_verbs,
            SetKind::FacetVerbs => &self.facet_verbs,
            SetKind::FacetlessVerbs => &self.facetless_verbs,
            SetKind::FacetInterfaces => &self.facet_interfaces,
            SetKind::UnservedVerbs => &self.unserved_verbs,
        }
    }
}

/// Derive all five sets from the IDL and the reference core's served-bit
/// constant. Every list is in **IDL document order**, which is what makes the
/// marker comparison an equality rather than a set comparison with a sort.
pub fn derive(protocol: &Protocol, grants_rs: &str) -> Result<DerivedSets> {
    let verb_enum = protocol
        .interface("vitrin_grant")
        .and_then(|i| i.enum_def("verb"))
        .ok_or_else(|| anyhow!("{XML_PATH} no longer defines `vitrin_grant.verb`"))?;

    let all_verbs: Vec<String> = verb_enum.entries.iter().map(|e| e.name.clone()).collect();

    let facet_interfaces: Vec<String> = protocol
        .interfaces
        .iter()
        .filter(|i| i.verb.is_some())
        .map(|i| i.name.clone())
        .collect();
    let declared: BTreeSet<&str> = protocol
        .interfaces
        .iter()
        .filter_map(|i| i.verb.as_deref())
        .collect();
    // Ordered by the VERB enum rather than by interface, so every list this
    // tool prints is in one order.
    let facet_verbs: Vec<String> = all_verbs
        .iter()
        .filter(|v| declared.contains(v.as_str()))
        .cloned()
        .collect();
    if facet_verbs.len() != declared.len() {
        bail!(
            "an `interface/@verb` names something that is not an entry of \
             `vitrin_grant.verb`: attribute values {declared:?}, verb entries {all_verbs:?}"
        );
    }
    let facetless_verbs: Vec<String> = all_verbs
        .iter()
        .filter(|v| !declared.contains(v.as_str()))
        .cloned()
        .collect();

    let served = parse_served_bits(grants_rs)?;
    let unserved_verbs: Vec<String> = verb_enum
        .entries
        .iter()
        .filter(|e| e.value & served == 0)
        .map(|e| e.name.clone())
        .collect();

    Ok(DerivedSets {
        all_verbs,
        facet_verbs,
        facetless_verbs,
        facet_interfaces,
        unserved_verbs,
    })
}

/// Pull `SERVED_VERB_BITS` out of the reference core's source.
///
/// Deliberately strict: the constant is a `|`-joined list of decimal literals
/// today, and anything else -- a named constant, a function call, a `!` -- is
/// an error rather than a silently-zero mask. A tool that guessed wrong here
/// would report every verb as unserved and look green on the carriers that say
/// so.
fn parse_served_bits(grants_rs: &str) -> Result<u32> {
    let needle = format!("{SERVED_BITS_CONST}: u32 = ");
    let rest = grants_rs.split(&needle).nth(1).ok_or_else(|| {
        anyhow!("{SERVED_BITS_PATH} no longer declares `{SERVED_BITS_CONST}: u32 = ...`")
    })?;
    let expr = rest
        .split(';')
        .next()
        .ok_or_else(|| anyhow!("`{SERVED_BITS_CONST}` has no terminating `;`"))?;
    let mut bits = 0u32;
    for term in expr.split('|') {
        let term = term.trim();
        let value = term.parse::<u32>().with_context(|| {
            format!(
                "`{SERVED_BITS_CONST}` is no longer a `|`-joined list of decimal literals \
                 (`{expr}`). Teach this parser the new shape rather than loosening it: a \
                 mask this tool reads as 0 makes every verb look unserved"
            )
        })?;
        bits |= value;
    }
    if bits == 0 {
        bail!("`{SERVED_BITS_CONST}` parsed to 0, which cannot be right");
    }
    Ok(bits)
}

/// The `<value>` names inside the schema's `interface/@verb` attribute
/// definition.
///
/// Deliberately narrow: it anchors on the `<attribute name="verb">` element
/// and reads to its close, so an unrelated `<value>` elsewhere in the schema
/// (there are several) cannot be mistaken for a verb.
fn rng_verb_values(rng: &str) -> Result<BTreeSet<&str>> {
    let after = rng
        .split(r#"<attribute name="verb">"#)
        .nth(1)
        .ok_or_else(|| anyhow!("no `<attribute name=\"verb\">` element"))?;
    let block = after
        .split("</attribute>")
        .next()
        .ok_or_else(|| anyhow!("`<attribute name=\"verb\">` is unterminated"))?;
    let mut out = BTreeSet::new();
    for chunk in block.split("<value>").skip(1) {
        let value = chunk
            .split("</value>")
            .next()
            .ok_or_else(|| anyhow!("a `<value>` in the verb attribute is unterminated"))?;
        out.insert(value.trim());
    }
    if out.is_empty() {
        bail!("the `interface/@verb` attribute admits no values at all");
    }
    Ok(out)
}

/// Parse every marker line in one file's text.
fn markers(text: &str) -> Result<Vec<Marker>> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let Some(after) = line.split(MARKER).nth(1) else {
            continue;
        };
        let line_no = i + 1;
        // Strip a trailing comment terminator (`-->`, `*/`) if present.
        let after = after
            .trim()
            .trim_end_matches("-->")
            .trim_end_matches("*/")
            .trim();
        let (decl, count_word) = match after.split_once('|') {
            Some((decl, count)) => {
                let word = count
                    .trim()
                    .strip_prefix("count:")
                    .ok_or_else(|| {
                        anyhow!("line {line_no}: the part after `|` must read `count: <word>`")
                    })?
                    .trim()
                    .to_string();
                (decl, Some(word))
            }
            None => (after, None),
        };
        let (kind, members) = decl.split_once('=').ok_or_else(|| {
            anyhow!("line {line_no}: a marker must read `{MARKER} <kind> = <name>, <name>...`")
        })?;
        let kind = SetKind::parse(kind.trim())
            .ok_or_else(|| anyhow!("line {line_no}: unknown set kind `{}`", kind.trim()))?;
        let members = members
            .split(',')
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
            .collect();
        out.push(Marker {
            kind,
            line_no,
            members,
            count_word,
        });
    }
    Ok(out)
}

/// How many lines on each side of a marker count as "the passage it sits on".
///
/// The check that every member is *named* has to be local, and the first cut
/// was not: it searched the whole file, and a levering pass showed why that is
/// nearly worthless. Deleting `egress` from the one sentence in
/// `docs/book/src/06-build-your-own-client.md` that enumerates the unserved
/// verbs left the check GREEN, because the word `egress` still appears three
/// lines further down in an unrelated sentence about bit allocation. Two of
/// round 2's four stale carriers would have passed a whole-file search for the
/// same reason.
///
/// Twenty is the smallest window every registered carrier passes: a table and
/// the sentence under it, a bulleted pair, a Rust doc comment, an XML
/// `<description>` stanza. Eight was tried and is too tight for five of them,
/// and the honest reading of that is that this check is a HEURISTIC and the
/// marker-equality check above it is not.
///
/// **What the window still cannot see**, stated because the alternative is
/// discovering it in a review: a mention of the same verb *within the window*
/// but in a different sentence satisfies it. Deleting `egress` from the one
/// enumerating sentence of `docs/book/src/06-build-your-own-client.md` is
/// still green here, because `egress` reappears six lines below in an
/// unrelated sentence about bit allocation. The same deletion in
/// `protocol/vitrin-v0.rng` IS red. So this catches a passage that forgot a
/// verb, and does not catch one that forgot a verb it happens to mention
/// again nearby. The check that carries the weight is the marker's list
/// equalling the derived set, which fires at every carrier whenever a set
/// moves and cannot be satisfied by an accident of wording.
const PASSAGE_LINES: usize = 20;

/// The `PASSAGE_LINES` lines either side of a marker, with every marker line
/// removed so a marker can never satisfy its own check.
fn passage_around(text: &str, line_no: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let idx = line_no.saturating_sub(1);
    let start = idx.saturating_sub(PASSAGE_LINES);
    let end = (idx + PASSAGE_LINES + 1).min(lines.len());
    lines[start..end]
        .iter()
        .filter(|l| !l.contains(MARKER))
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every spelling a surface may legitimately use for one member.
///
/// The wire name always counts; so does the **SDK-level dotted name**, which
/// the IDL fixes rather than leaves to be invented (replace the first
/// underscore with a dot; a name with no underscore is unchanged). The book
/// and the Python SDK address agent authors and use the dotted form
/// throughout, and forcing `observe_cursor` into a page whose whole register
/// is `observe.cursor` would be this check dictating prose, which is the
/// failure mode `limits-check`'s own docs warn about. Case is ignored by
/// [`contains_word`], so `OBSERVE_CURSOR` matches too.
fn mention_forms(member: &str) -> Vec<String> {
    let mut forms = vec![member.to_string()];
    if let Some(pos) = member.find('_') {
        let mut dotted = member.to_string();
        dotted.replace_range(pos..pos + 1, ".");
        forms.push(dotted);
    }
    forms
}

/// Whether `needle` occurs in `haystack` bounded by non-word characters, so
/// `two` does not match inside `network` and `six` does not match `sixty`.
fn contains_word(haystack: &str, needle: &str) -> bool {
    let lower = haystack.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut from = 0;
    while let Some(pos) = lower[from..].find(&needle) {
        let start = from + pos;
        let end = start + needle.len();
        let before_ok = start == 0 || !is_word_byte(bytes[start - 1]);
        let after_ok = end == bytes.len() || !is_word_byte(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Run the whole check against a workspace root. Reads only; writes nothing.
pub fn check(root: &Path) -> Result<String> {
    let xml_path = root.join(XML_PATH);
    let xml = std::fs::read_to_string(&xml_path)
        .with_context(|| format!("reading {}", xml_path.display()))?;
    let protocol = vitrin_scanner::parse::parse(&xml)
        .with_context(|| format!("parsing {}", xml_path.display()))?;
    let grants_path = root.join(SERVED_BITS_PATH);
    let grants_rs = std::fs::read_to_string(&grants_path)
        .with_context(|| format!("reading {}", grants_path.display()))?;
    let sets = derive(&protocol, &grants_rs)?;

    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;

    // The schema's own closed `@verb` value set, held to the IDL in the one
    // direction `xmllint` cannot see. `xmllint --relaxng` already refuses an
    // IDL that uses a verb name the schema omits; nothing refuses a schema
    // that admits a name no interface uses, and a value set that had drifted
    // wider than the facets would silently re-admit exactly the mutation
    // `protocol/test-mutations.sh`'s `verb-without-facet` case exists to
    // prove is refused.
    let rng_path = root.join(RNG_PATH);
    match std::fs::read_to_string(&rng_path) {
        Ok(rng) => match rng_verb_values(&rng) {
            Ok(values) => {
                let expected: BTreeSet<&str> =
                    sets.facet_verbs.iter().map(String::as_str).collect();
                if values != expected {
                    failures.push(format!(
                        "{RNG_PATH}: the closed `interface/@verb` value set is {values:?}; \
                         the facet verbs the IDL declares are {expected:?}"
                    ));
                }
            }
            Err(e) => failures.push(format!("{RNG_PATH}: {e}")),
        },
        Err(e) => failures.push(format!("{RNG_PATH}: unreadable ({e})")),
    }

    for (rel, kind) in CARRIERS {
        let path = root.join(rel);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                failures.push(format!(
                    "{rel}: registered as a `{kind}` carrier but unreadable ({e}). \
                     Delete the CARRIERS row if the surface is gone."
                ));
                continue;
            }
        };
        let found = match markers(&text) {
            Ok(m) => m,
            Err(e) => {
                failures.push(format!("{rel}: {e}"));
                continue;
            }
        };
        let of_kind: Vec<&Marker> = found.iter().filter(|m| m.kind == *kind).collect();
        if of_kind.is_empty() {
            failures.push(format!(
                "{rel}: registered as a `{kind}` carrier and carries no `{MARKER} {kind} = ...` \
                 marker. Either restore it or delete the CARRIERS row -- a registry entry with \
                 no marker checks nothing."
            ));
            continue;
        }
        let expected = sets.get(*kind);
        for marker in of_kind {
            checked += 1;
            if marker.members != expected {
                failures.push(format!(
                    "{rel}:{}: the `{kind}` marker lists {:?}; the set derived from the IDL \
                     (and, for unserved-verbs, from {SERVED_BITS_PATH}) is {expected:?}. \
                     Fix the prose this marker sits on, then the marker.",
                    marker.line_no, marker.members
                ));
                continue;
            }
            let passage = passage_around(&text, marker.line_no);
            for member in expected {
                if !mention_forms(member)
                    .iter()
                    .any(|f| contains_word(&passage, f))
                {
                    failures.push(format!(
                        "{rel}:{}: the `{kind}` marker is correct but the {PASSAGE_LINES} lines \
                         around it never name `{member}` (in any of {:?}). The marker is not the \
                         enumeration -- the passage it sits on is, and that is what a reader \
                         believes. Either the passage went stale, or the marker drifted away \
                         from the passage it describes and should be moved back onto it.",
                        marker.line_no,
                        mention_forms(member)
                    ));
                }
            }
            if let Some(word) = &marker.count_word {
                let expected_word = NUMBER_WORDS.get(expected.len()).ok_or_else(|| {
                    anyhow!(
                        "no English word for a set of {} -- extend NUMBER_WORDS",
                        expected.len()
                    )
                })?;
                if !word.eq_ignore_ascii_case(expected_word) {
                    failures.push(format!(
                        "{rel}:{}: the `{kind}` marker states the count word `{word}`; \
                         the set has {} member(s), so the word is `{expected_word}`.",
                        marker.line_no,
                        expected.len()
                    ));
                } else if !contains_word(&passage, expected_word) {
                    failures.push(format!(
                        "{rel}:{}: the `{kind}` count word `{expected_word}` appears nowhere in \
                         the {PASSAGE_LINES} lines around the marker. A count declared only in a \
                         marker is a count nobody reads.",
                        marker.line_no
                    ));
                }
            }
        }
    }

    // A marker outside the registry is red too: it is either a new carrier
    // nobody registered, or a typo'd kind in a file that believes it is
    // covered.
    let registered: BTreeSet<PathBuf> = CARRIERS.iter().map(|(rel, _)| root.join(rel)).collect();
    for path in files_with_markers(root)? {
        if !registered.contains(&path) {
            let rel = path.strip_prefix(root).unwrap_or(&path).display();
            failures.push(format!(
                "{rel}: carries a `{MARKER}` marker and is not in CARRIERS, so nothing checks \
                 it. Add the (path, kind) row."
            ));
        }
    }

    if !failures.is_empty() {
        bail!(
            "verb-set drift ({} problem(s)):\n  - {}",
            failures.len(),
            failures.join("\n  - ")
        );
    }
    Ok(format!(
        "verb-sets: {checked} marker(s) across {} registered carrier(s) agree with the IDL.\n  \
         all-verbs        = {:?}\n  facet-verbs      = {:?}\n  facetless-verbs  = {:?}\n  \
         facet-interfaces = {:?}\n  unserved-verbs   = {:?}",
        CARRIERS.len(),
        sets.all_verbs,
        sets.facet_verbs,
        sets.facetless_verbs,
        sets.facet_interfaces,
        sets.unserved_verbs,
    ))
}

/// Every file under `root` that carries a marker, skipping build output and
/// VCS metadata and this module itself (whose doc comment quotes the marker
/// syntax).
///
/// # Why git decides what is build output, and not a name
///
/// This scan first carried a hand-written list of directory names to skip --
/// `target`, `.git`, `node_modules`, `build`, `dist`, `*.egg-info` -- and the
/// list was wrong the day it landed. `mdbook build docs/book` writes into
/// `docs/book/book/`, whose name is `book`, so a tree where one of this
/// branch's own claimed gates had been run reported four unregisterable
/// carriers (`docs/book/book/{02,03,06}-*.html` and `print.html`) and turned
/// both `cargo xtask verb-sets --check` and `cargo test --workspace` red.
/// Every one of those paths is in `.gitignore`; none of them exists on a
/// fresh clone.
///
/// The list was already the second attempt at the same shape -- `build`,
/// `dist` and `*.egg-info` were added because `pip install ./sdk/python`
/// drops a copy of every SDK source, markers and all, under
/// `sdk/python/build/lib/`. Adding `book` would have been the third, and the
/// next generated directory would be the fourth. So the name-shaped answer is
/// gone: [`BuildOutput`] asks git which paths are ignored, which is the same
/// answer `.gitignore` gives the developer and the same tree CI checks out,
/// and it is already how [`crate::limits`], [`crate::skip_census`] and
/// [`crate::test_census`] tell source from output (issue #295).
///
/// `.git` stays a literal name because it is the one directory git does not
/// report as ignored -- it is not in the working tree at all.
///
/// What this deliberately does **not** skip is a file that is untracked and
/// *not* ignored: a new carrier a developer has written but not yet committed
/// is exactly the file this check must speak about.
fn files_with_markers(root: &Path) -> Result<Vec<PathBuf>> {
    let built = BuildOutput::of_tree(root).with_context(|| {
        format!(
            "asking git which paths under {} are build output. This scan reports every file \
             carrying a `{MARKER}` marker that no CARRIERS row covers, and a generated copy of \
             a carrier is not a surface anyone can register -- see \
             crates/xtask/src/build_output.rs (issue #295).",
            root.display()
        )
    })?;
    let mut out = Vec::new();
    let this_file = root.join("crates/xtask/src/verb_sets.rs");
    walk(root, &built, &mut |path| {
        if path == this_file {
            return Ok(());
        }
        // Read as bytes: the tree carries PNGs and other binaries, and a
        // non-UTF-8 file simply has no marker.
        let Ok(bytes) = std::fs::read(path) else {
            return Ok(());
        };
        if let Ok(text) = std::str::from_utf8(&bytes) {
            if text.contains(MARKER) {
                out.push(path.to_path_buf());
            }
        }
        Ok(())
    })?;
    out.sort();
    Ok(out)
}

fn walk(dir: &Path, built: &BuildOutput, f: &mut impl FnMut(&Path) -> Result<()>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // `.git` is not part of the working tree, so git never reports it as
        // ignored; everything else that is output says so through git.
        if name == ".git" || built.covers(&path) {
            continue;
        }
        if entry.file_type()?.is_dir() {
            walk(&path, built, f)?;
        } else if entry.file_type()?.is_file() {
            f(&path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testtree::TestTree;

    fn root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root")
    }

    /// The gate itself. Lives as a test as well as a subcommand so that
    /// `cargo test --workspace` -- which CI already runs -- fails on drift
    /// even before anyone wires the subcommand into a job.
    #[test]
    fn every_registered_surface_agrees_with_the_idl() {
        match check(&root()) {
            Ok(report) => eprintln!("{report}"),
            Err(e) => panic!("{e:?}"),
        }
    }

    #[test]
    fn the_registry_covers_every_set_kind() {
        // A kind with no carrier is a kind whose whole point has quietly
        // lapsed, and the tool would still report success.
        for kind in [
            SetKind::AllVerbs,
            SetKind::FacetVerbs,
            SetKind::FacetlessVerbs,
            SetKind::FacetInterfaces,
            SetKind::UnservedVerbs,
        ] {
            assert!(
                CARRIERS.iter().any(|(_, k)| *k == kind),
                "no registered carrier enumerates `{kind}`"
            );
        }
    }

    #[test]
    fn the_derivation_is_not_vacuous() {
        let root = root();
        let xml = std::fs::read_to_string(root.join(XML_PATH)).expect("the IDL");
        let protocol = vitrin_scanner::parse::parse(&xml).expect("the IDL parses");
        let grants = std::fs::read_to_string(root.join(SERVED_BITS_PATH)).expect("grants.rs");
        let sets = derive(&protocol, &grants).expect("derivation");

        // The three properties that make the five lists a partition rather
        // than five unrelated lists.
        assert_eq!(
            sets.facet_verbs.len() + sets.facetless_verbs.len(),
            sets.all_verbs.len()
        );
        assert_eq!(sets.facet_verbs.len(), sets.facet_interfaces.len());
        assert!(!sets.unserved_verbs.is_empty());
        assert!(sets.unserved_verbs.len() < sets.all_verbs.len());
        // Document order is preserved, so the marker comparison is an
        // equality and not a sort-then-compare.
        assert_eq!(sets.all_verbs.first().map(String::as_str), Some("observe"));
    }

    #[test]
    fn a_stale_marker_is_a_failure() {
        // Non-vacuity for the whole instrument: the same comparison the check
        // runs, against a marker that lists the set as it read before
        // `egress` landed.
        let stale = "<!-- vitrin-verb-set: facetless-verbs = observe_cursor -->";
        let parsed = markers(stale).expect("parses");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].members, ["observe_cursor"]);
        assert_ne!(parsed[0].members, ["observe_cursor", "egress"]);
    }

    #[test]
    fn a_marker_cannot_satisfy_its_own_passage_check() {
        let text = "<!-- vitrin-verb-set: facetless-verbs = observe_cursor, egress -->\nprose\n";
        assert!(!passage_around(text, 1).contains("observe_cursor"));
    }

    #[test]
    fn the_passage_is_local_to_the_marker() {
        // The whole point of the window: a mention far away in the same file
        // must not stand in for the enumeration the marker sits on. This is
        // the lever that found the first cut of this check to be nearly
        // worthless -- deleting `egress` from the one sentence that
        // enumerated it left a whole-file search green, because the word
        // appears again three lines below in a sentence about bit values.
        let mut lines: Vec<String> = vec!["// vitrin-verb-set: all-verbs = a".into()];
        lines.push("near".into());
        for _ in 0..PASSAGE_LINES * 2 {
            lines.push("filler".into());
        }
        lines.push("far".into());
        let text = lines.join("\n");
        let passage = passage_around(&text, 1);
        assert!(contains_word(&passage, "near"));
        assert!(!contains_word(&passage, "far"));
    }

    #[test]
    fn count_words_are_matched_on_word_boundaries() {
        assert!(contains_word("there are Two remaining", "two"));
        assert!(!contains_word("the network is unreachable", "two"));
        assert!(!contains_word("sixty surfaces", "six"));
        assert!(contains_word("Six verbs map", "six"));
        // A verb name inside a longer identifier is not a mention of it.
        assert!(!contains_word("observe_cursor_delivery", "observe_cursor"));
        assert!(contains_word("`observe_cursor` has none", "observe_cursor"));
    }

    #[test]
    fn the_dotted_sdk_spelling_counts_as_a_mention() {
        assert_eq!(
            mention_forms("observe_cursor"),
            ["observe_cursor", "observe.cursor"]
        );
        assert_eq!(
            mention_forms("actuate_pointer"),
            ["actuate_pointer", "actuate.pointer"]
        );
        // The IDL states the degenerate case explicitly: a wire name with no
        // underscore has nothing to replace, and `egress` is the first.
        assert_eq!(mention_forms("egress"), ["egress"]);
        // Only the FIRST underscore becomes a dot.
        assert_eq!(mention_forms("a_b_c"), ["a_b_c", "a.b_c"]);
    }

    #[test]
    fn the_schema_value_set_reader_finds_only_verb_values() {
        let rng = std::fs::read_to_string(root().join(RNG_PATH)).expect("the schema");
        let values = rng_verb_values(&rng).expect("verb values");
        assert!(values.contains("observe"));
        assert!(values.contains("realm_launch"));
        // The schema has `<value>` elements outside the verb attribute (the
        // `destructor` message type, for one); none of them may leak in.
        assert!(!values.contains("destructor"));
        assert!(!values.contains("observe_cursor"));
        assert!(rng_verb_values("<grammar/>").is_err());
    }

    #[test]
    fn served_bits_parsing_refuses_a_shape_it_does_not_understand() {
        assert_eq!(
            parse_served_bits("pub(crate) const SERVED_VERB_BITS: u32 = 1 | 2 | 4;").unwrap(),
            7
        );
        assert!(parse_served_bits("const SERVED_VERB_BITS: u32 = OTHER | 2;").is_err());
        assert!(parse_served_bits("nothing here").is_err());
        assert!(parse_served_bits("const SERVED_VERB_BITS: u32 = 0;").is_err());
    }

    /// The regression for the defect that replaced the hand-written skip
    /// list: a marker inside a **gitignored** directory is not a carrier, and
    /// the directory that proved it is `docs/book/book/` -- mdBook's output,
    /// named `book`, which no entry of that list covered.
    ///
    /// Deterministic, in a tree this test builds, for the reason
    /// [`crate::testtree`] exists: held against the real repository the
    /// assertion would pass on a clean checkout whether or not the fix was
    /// there. The second half is the one that keeps the filter honest -- an
    /// untracked file git does *not* ignore is still reported, so this cannot
    /// be satisfied by a scan that has silently switched itself off.
    #[test]
    fn a_marker_in_build_output_is_not_a_carrier_whatever_the_directory_is_called() {
        let tree = TestTree::new("verb-sets-build-output");
        tree.write(".gitignore", "docs/book/book/\nsdk/python/build/\n");
        let marker = "<!-- vitrin-verb-set: facetless-verbs = observe_cursor, egress -->\n";
        // What `mdbook build docs/book` writes, and what `pip install
        // ./sdk/python` copies: the same marker text, at a generated path.
        tree.write("docs/book/book/03-grants-consent-revocation.html", marker);
        tree.write("sdk/python/build/lib/vitrin/grants.py", marker);
        // The repository's own text, and a carrier a developer has written
        // but not yet committed.
        tree.write("docs/book/src/03-grants-consent-revocation.md", marker);
        tree.write("docs/protocol/99-vitrin_new.md", marker);
        tree.git_init();

        let found: Vec<PathBuf> = files_with_markers(tree.path())
            .expect("a git work tree")
            .into_iter()
            .map(|p| p.strip_prefix(tree.path()).expect("under the tree").into())
            .collect();
        assert_eq!(
            found,
            vec![
                PathBuf::from("docs/book/src/03-grants-consent-revocation.md"),
                PathBuf::from("docs/protocol/99-vitrin_new.md"),
            ],
            "generated copies are not surfaces anyone can register; uncommitted source is"
        );
    }

    #[test]
    fn a_marker_with_a_bad_count_word_is_rejected_by_shape() {
        let m = markers("# vitrin-verb-set: unserved-verbs = a, b | count: two").expect("parses");
        assert_eq!(m[0].count_word.as_deref(), Some("two"));
        assert!(markers("# vitrin-verb-set: unserved-verbs = a | nope: two").is_err());
        assert!(markers("# vitrin-verb-set: not-a-kind = a").is_err());
        assert!(markers("# vitrin-verb-set: all-verbs").is_err());
    }
}
