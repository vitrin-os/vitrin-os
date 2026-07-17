//! XML -> IR parsing, using `roxmltree`.
//!
//! This module assumes the input has already been validated against
//! `protocol/vitrin-v0.rng` (a separate CI step via `xmllint`, out of scope here).
//! It does not re-validate schema structure; it does perform the semantic
//! checks the RNG schema cannot express -- string-arg max-byte tokens,
//! enum/interface reference resolution, name uniqueness, the one-fd-per-message
//! framing invariant, the u8 opcode space, and the u16 frame-size budget --
//! because those are exactly the facts a codegen backend needs and cannot
//! safely default or guess at. Every one of these must fail loudly *here*:
//! the alternative is a confusing compile error deep inside generated code,
//! or worse, generated marshal code that is silently wrong on the wire.
//!
//! It also *rejects* (rather than silently drops) the RNG dialect's
//! version-growth vocabulary -- `since`, `deprecated-since`,
//! `type="destructor"` -- which the conventions doc plans for version 2 but
//! no backend implements yet. Accepting-and-ignoring those attributes would
//! emit a `since="2"` message as an unconditional version-1 one with zero
//! warnings, which is exactly the failure mode this module exists to prevent.

use std::sync::LazyLock;

use anyhow::{anyhow, bail, Context, Result};
use roxmltree::{Document, Node};

use crate::casing::to_pascal_case;
use crate::ir::{Arg, ArgType, EnumDef, EnumEntry, EnumRef, Interface, Message, Protocol};

static MAX_BYTES_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\(max\s+(\d+)\s+bytes\)").expect("static regex is valid"));

/// Parse a complete protocol document from XML source text.
pub fn parse(xml: &str) -> Result<Protocol> {
    let doc = Document::parse(xml).context("XML is not well-formed")?;
    let root = doc.root_element();
    if root.tag_name().name() != "protocol" {
        bail!(
            "root element is <{}>, expected <protocol>",
            root.tag_name().name()
        );
    }

    let name = req_attr(root, "name")?.to_string();
    let version: u32 = req_attr(root, "version")?
        .parse()
        .context("protocol/@version is not a valid integer")?;

    let mut interfaces: Vec<Interface> = Vec::new();
    for child in root.children().filter(|n| n.is_element()) {
        match child.tag_name().name() {
            "interface" => {
                let iface = parse_interface(child)?;
                if interfaces.iter().any(|i| i.name == iface.name) {
                    bail!(
                        "duplicate interface name '{}': the one-file-per-interface Rust \
                         backend would silently overwrite the first definition's module \
                         with the second, and enum references would resolve against \
                         whichever definition happened to come first",
                        iface.name
                    );
                }
                interfaces.push(iface);
            }
            // The protocol element's own <description> and <copyright> are
            // prose for humans; no backend consumes them.
            "description" | "copyright" => {}
            other => bail!("unexpected protocol-level child element <{other}>"),
        }
    }
    if interfaces.is_empty() {
        bail!("protocol document defines zero interfaces");
    }

    let protocol = Protocol {
        name,
        version,
        interfaces,
    };

    validate_enum_refs(&protocol)?;
    validate_interface_refs(&protocol)?;

    Ok(protocol)
}

fn parse_interface(node: Node) -> Result<Interface> {
    let name = req_attr(node, "name")?.to_string();
    let version: u32 = req_attr(node, "version")?
        .parse()
        .with_context(|| format!("interface '{name}': @version is not a valid integer"))?;
    let verb = node.attribute("verb").map(|s| s.to_string());
    let summary = description_summary(node).with_context(|| format!("interface '{name}'"))?;

    let mut requests = Vec::new();
    let mut events = Vec::new();
    let mut enums = Vec::new();

    for child in node.children().filter(|n| n.is_element()) {
        match child.tag_name().name() {
            "request" => {
                let opcode = checked_opcode(&name, "request", requests.len())?;
                requests.push(
                    parse_message(child, opcode, &name)
                        .with_context(|| format!("interface '{name}', request #{opcode}"))?,
                );
            }
            "event" => {
                let opcode = checked_opcode(&name, "event", events.len())?;
                events.push(
                    parse_message(child, opcode, &name)
                        .with_context(|| format!("interface '{name}', event #{opcode}"))?,
                );
            }
            "enum" => {
                enums.push(parse_enum(child).with_context(|| format!("interface '{name}'"))?);
            }
            "description" => {} // already consumed above
            other => bail!("interface '{name}': unexpected child element <{other}>"),
        }
    }

    check_unique_messages(&name, &requests, "request")?;
    check_unique_messages(&name, &events, "event")?;
    for (i, e) in enums.iter().enumerate() {
        if enums[..i].iter().any(|p| p.name == e.name) {
            bail!("interface '{name}': duplicate enum name '{}'", e.name);
        }
    }

    Ok(Interface {
        name,
        version,
        verb,
        summary,
        requests,
        events,
        enums,
    })
}

/// Guard opcode assignment against the wire's `u8` opcode space. Without
/// this, `len() as u8` on a 257th message would silently wrap, handing
/// message #256 opcode 0 again -- a wire-ambiguous protocol generated with
/// no error anywhere.
fn checked_opcode(iface_name: &str, kind: &str, index: usize) -> Result<u8> {
    u8::try_from(index).map_err(|_| {
        anyhow!(
            "interface '{iface_name}' defines more than {} {kind}s; opcodes are a u8 \
             on the wire, so this cannot be represented",
            u8::MAX as usize + 1
        )
    })
}

/// Reject the RNG dialect's version-growth attributes until a backend
/// actually implements them. Silently dropping `since="2"` would emit the
/// item as an unconditional version-1 one -- see the module doc comment.
fn reject_unsupported_growth_attrs(node: Node, what: &str) -> Result<()> {
    for attr in ["since", "deprecated-since"] {
        if let Some(v) = node.attribute(attr) {
            bail!(
                "{what} carries {attr}=\"{v}\": version-gated growth is not implemented \
                 by any codegen backend yet, and silently ignoring it would emit this \
                 item unconditionally at version 1 -- implement support before using it"
            );
        }
    }
    Ok(())
}

/// One argument's worst-case contribution to an encoded frame, in bytes:
/// scalars are 4, strings are a 4-byte length prefix plus the documented
/// maximum padded to 4, fds contribute nothing to the byte buffer.
fn arg_worst_case_wire_size(arg: &Arg) -> u64 {
    match &arg.ty {
        ArgType::Fd => 0,
        ArgType::String { max_bytes } => 4 + u64::from(*max_bytes).div_ceil(4) * 4,
        _ => 4,
    }
}

fn parse_message(node: Node, opcode: u8, own_interface: &str) -> Result<Message> {
    let name = req_attr(node, "name")?.to_string();
    reject_unsupported_growth_attrs(node, &format!("message '{name}'"))?;
    if let Some(ty) = node.attribute("type") {
        bail!(
            "message '{name}' declares type=\"{ty}\": destructor semantics are not \
             implemented by any codegen backend yet, and silently ignoring the attribute \
             would emit this as an ordinary message -- implement support before using it"
        );
    }
    let summary = description_summary(node).with_context(|| format!("message '{name}'"))?;

    let mut args = Vec::new();
    for child in node.children().filter(|n| n.is_element()) {
        match child.tag_name().name() {
            "arg" => {
                args.push(parse_arg(child, own_interface).with_context(|| {
                    format!(
                        "message '{name}', arg '{}'",
                        child.attribute("name").unwrap_or("?")
                    )
                })?);
            }
            "description" => {} // already consumed above
            other => bail!("message '{name}': unexpected child element <{other}>"),
        }
    }

    for (i, a) in args.iter().enumerate() {
        if args[..i].iter().any(|p| p.name == a.name) {
            bail!("message '{name}': duplicate arg name '{}'", a.name);
        }
    }

    // The one-fd-per-message framing invariant (docs/protocol/00-conventions.md
    // 2.4): the header's fd_count field is 0 or 1, full stop. The RNG cannot
    // express this; both backends' fd handling (a boolean `has_fd`, a single
    // out-of-band fd parameter) silently assumes it.
    let fd_count = args.iter().filter(|a| matches!(a.ty, ArgType::Fd)).count();
    if fd_count > 1 {
        bail!(
            "message '{name}' declares {fd_count} fd arguments; the wire's fd_count \
             header field is 0 or 1 (at most one fd per message is a framing invariant \
             -- multi-fd needs arrive as builder patterns, one fd per message)"
        );
    }

    // The u16 frame-size budget: the encoders' frame-size assertions
    // (`patch_size`'s assert on the Rust side, the total-size check on the C
    // side) lean on every message fitting 65535 bytes *by construction*.
    // Enforce that construction mechanically: a legal-looking string-bound
    // bump in the IDL must fail here, not become a reachable encode panic on
    // per-field-conformant input.
    let worst_case = 8 + args.iter().map(arg_worst_case_wire_size).sum::<u64>();
    if worst_case > u64::from(u16::MAX) {
        bail!(
            "message '{name}': worst-case frame size is {worst_case} bytes (8-byte header \
             + every argument at its documented maximum), exceeding the wire's 65535-byte \
             u16 size field -- shrink a string bound or split the message"
        );
    }

    Ok(Message {
        name,
        opcode,
        summary,
        args,
    })
}

fn parse_arg(node: Node, own_interface: &str) -> Result<Arg> {
    let name = req_attr(node, "name")?.to_string();
    let summary = node.attribute("summary").unwrap_or("").to_string();
    let allow_null = node.attribute("allow-null") == Some("true");
    let ty_str = req_attr(node, "type")?;

    let ty = match ty_str {
        "int" => ArgType::Int {
            enum_ref: parse_enum_ref(node, own_interface)?,
        },
        "uint" => ArgType::Uint {
            enum_ref: parse_enum_ref(node, own_interface)?,
        },
        "fixed" => ArgType::Fixed,
        "fd" => ArgType::Fd,
        "string" => {
            let mut captures = MAX_BYTES_RE.captures_iter(&summary);
            let first = captures.next().ok_or_else(|| {
                anyhow!(
                    "string arg '{name}' has no '(max N bytes)' token in its summary \
                     (summary was: {summary:?}) -- every string arg must document a bound"
                )
            })?;
            // The IDL's normative STRING BOUNDS text requires *exactly one*
            // token; silently binding the first of several would let an
            // edited summary like "legacy (max 256 bytes), now (max 1024
            // bytes)" pick the wrong bound with no error.
            if captures.next().is_some() {
                bail!(
                    "string arg '{name}' has multiple '(max N bytes)' tokens in its \
                     summary (summary was: {summary:?}) -- exactly one is required"
                );
            }
            let max_bytes = first
                .get(1)
                .expect("regex has one capture group")
                .as_str()
                .parse::<u32>()
                .context("max-bytes token did not parse as u32")?;
            if max_bytes == 0 {
                bail!(
                    "string arg '{name}' documents '(max 0 bytes)', which would make \
                     every value empty -- a real bound is required"
                );
            }
            ArgType::String { max_bytes }
        }
        "object" => ArgType::Object {
            interface: req_attr(node, "interface")?.to_string(),
        },
        "new_id" => ArgType::NewId {
            interface: req_attr(node, "interface")?.to_string(),
        },
        other => bail!("arg '{name}': unknown type '{other}'"),
    };

    validate_arg_name(&name, matches!(ty, ArgType::Fd))?;

    Ok(Arg {
        name,
        summary,
        ty,
        allow_null,
    })
}

/// Rust and C keywords, merged: an arg name is emitted verbatim as a struct
/// field (both backends) and as a `let` binding (Rust decode), so a keyword
/// produces uncompilable generated code with a confusing diagnostic far from
/// the actual mistake. (`r#`-escaping is possible in Rust but has no C
/// equivalent, so rejection is the honest cross-backend policy.)
const RESERVED_KEYWORDS: &[&str] = &[
    // Rust (strict + reserved-in-practice)
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "crate",
    "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "if", "impl", "in",
    "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref",
    "return", "self", "static", "struct", "super", "trait", "true", "try", "type", "typeof",
    "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
    // C11 (those not already listed above)
    "auto", "case", "char", "default", "double", "float", "goto", "inline", "int", "long",
    "register", "restrict", "short", "signed", "sizeof", "switch", "typedef", "union", "unsigned",
    "void", "volatile",
];

/// Local variable names the Rust decode template binds before/while decoding
/// arguments (`fn decode(bytes, fd)` parameters, the parsed `header`, the
/// cursor `pos`). A generated `let <arg> = ...` for an arg with one of these
/// names would shadow the local mid-function: `bytes`/`header` fail to
/// compile (type mismatch, a diagnostic pointing into generated code), but a
/// shadowed `pos` is the dangerous one -- subsequent reads would silently
/// use a `u32` field value as the cursor if the types happened to line up.
/// `fd` is special-cased in [`validate_arg_name`]: the fd-typed argument
/// itself may be (and in v0 is) named `fd`, because the template's
/// `let fd = fd.expect(..)` consumes the parameter exactly then.
const RESERVED_TEMPLATE_LOCALS: &[&str] = &["bytes", "header", "pos"];

fn validate_arg_name(name: &str, is_fd_typed: bool) -> Result<()> {
    if RESERVED_KEYWORDS.contains(&name) {
        bail!(
            "arg name '{name}' is a Rust or C keyword and cannot be emitted as a \
             field/binding name in generated code -- rename it"
        );
    }
    if RESERVED_TEMPLATE_LOCALS.contains(&name) {
        bail!(
            "arg name '{name}' collides with a local variable the generated Rust decode \
             uses internally and would shadow it mid-decode -- rename it"
        );
    }
    if name == "fd" && !is_fd_typed {
        bail!(
            "arg name 'fd' is reserved for fd-typed arguments (it would shadow the \
             generated decode's out-of-band fd parameter) -- rename it"
        );
    }
    Ok(())
}

/// Resolve an `enum="..."` attribute (legal only on `int`/`uint` args) to a
/// concrete `(interface, name)` pair. A bare name resolves against
/// `own_interface`; a dotted `iface.name` resolves against the named interface.
fn parse_enum_ref(node: Node, own_interface: &str) -> Result<Option<EnumRef>> {
    let Some(raw) = node.attribute("enum") else {
        return Ok(None);
    };
    Ok(Some(match raw.split_once('.') {
        Some((iface, name)) => EnumRef {
            interface: iface.to_string(),
            name: name.to_string(),
        },
        None => EnumRef {
            interface: own_interface.to_string(),
            name: raw.to_string(),
        },
    }))
}

fn parse_enum(node: Node) -> Result<EnumDef> {
    let name = req_attr(node, "name")?.to_string();
    reject_unsupported_growth_attrs(node, &format!("enum '{name}'"))?;
    let bitfield = node.attribute("bitfield") == Some("true");
    let summary = description_summary(node).with_context(|| format!("enum '{name}'"))?;

    let mut entries = Vec::new();
    for child in node.children().filter(|n| n.is_element()) {
        match child.tag_name().name() {
            "entry" => entries.push(parse_entry(child).with_context(|| format!("enum '{name}'"))?),
            "description" => {} // already consumed above
            other => bail!("enum '{name}': unexpected child element <{other}>"),
        }
    }
    if entries.is_empty() {
        bail!("enum '{name}' defines zero entries");
    }
    for (i, e) in entries.iter().enumerate() {
        // Name identity is the *PascalCase* form, because that is what the
        // Rust backend emits as the variant name -- `foo_bar` and `foo__bar`
        // are distinct XML names that collide in generated code.
        let pascal = to_pascal_case(&e.name);
        if let Some(prev) = entries[..i]
            .iter()
            .find(|p| to_pascal_case(&p.name) == pascal)
        {
            bail!(
                "enum '{name}': entries '{}' and '{}' both become '{pascal}' in \
                 generated Rust",
                prev.name,
                e.name
            );
        }
        if let Some(prev) = entries[..i].iter().find(|p| p.value == e.value) {
            bail!(
                "enum '{name}': entries '{}' and '{}' share the value {} -- every \
                 entry's wire value must be distinct",
                prev.name,
                e.name,
                e.value
            );
        }
    }

    Ok(EnumDef {
        name,
        bitfield,
        summary,
        entries,
    })
}

fn parse_entry(node: Node) -> Result<EnumEntry> {
    let name = req_attr(node, "name")?.to_string();
    reject_unsupported_growth_attrs(node, &format!("entry '{name}'"))?;
    let value_str = req_attr(node, "value")?;
    let value = parse_enum_value(value_str).with_context(|| {
        format!("entry '{name}': value {value_str:?} is not valid decimal or 0x-hex")
    })?;
    let summary = node.attribute("summary").unwrap_or("").to_string();

    Ok(EnumEntry {
        name,
        value,
        summary,
    })
}

fn parse_enum_value(s: &str) -> Result<u32> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16).map_err(Into::into)
    } else {
        s.parse::<u32>().map_err(Into::into)
    }
}

/// Messages within one kind (requests, or events) must be unique -- like enum
/// entries, name identity is the PascalCase form the Rust backend emits as
/// the struct name. A request and an event MAY share a name (they live in
/// disjoint namespaces in both backends: `requests::`/`events::` submodules
/// in Rust, a `req`/`evt` infix in C).
fn check_unique_messages(iface_name: &str, list: &[Message], kind: &str) -> Result<()> {
    for (i, m) in list.iter().enumerate() {
        let pascal = to_pascal_case(&m.name);
        if let Some(prev) = list[..i].iter().find(|p| to_pascal_case(&p.name) == pascal) {
            bail!(
                "interface '{iface_name}': {kind}s '{}' and '{}' both become '{pascal}' \
                 in generated Rust",
                prev.name,
                m.name
            );
        }
    }
    Ok(())
}

/// Every `interface`, `request`/`event`, and `enum` element carries a required
/// `<description summary="...">` child (schema-enforced); the summary is what
/// the backends embed as doc comments. The element's *text body* (long-form
/// prose) is for human readers of the XML and is not extracted.
fn description_summary(parent: Node) -> Result<String> {
    let desc = parent
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "description")
        .ok_or_else(|| anyhow!("missing required <description> child"))?;
    req_attr(desc, "summary").map(|s| s.to_string())
}

fn req_attr<'a>(node: Node<'a, 'a>, attr: &str) -> Result<&'a str> {
    node.attribute(attr).ok_or_else(|| {
        anyhow!(
            "<{}> is missing required attribute '{attr}'",
            node.tag_name().name()
        )
    })
}

/// Cross-check every resolved `EnumRef` actually names an enum that exists.
/// The RNG schema cannot express this (it only constrains the attribute's
/// lexical shape); catching a typo here beats catching it as a Rust compile
/// error deep inside generated code.
fn validate_enum_refs(protocol: &Protocol) -> Result<()> {
    for iface in &protocol.interfaces {
        for msg in iface.requests.iter().chain(iface.events.iter()) {
            for arg in &msg.args {
                let enum_ref = match &arg.ty {
                    ArgType::Int { enum_ref } | ArgType::Uint { enum_ref } => enum_ref.as_ref(),
                    _ => None,
                };
                let Some(enum_ref) = enum_ref else { continue };
                let target_iface = protocol.interface(&enum_ref.interface).ok_or_else(|| {
                    anyhow!(
                        "{}.{}: arg '{}' references enum on unknown interface '{}'",
                        iface.name,
                        msg.name,
                        arg.name,
                        enum_ref.interface
                    )
                })?;
                if target_iface.enum_def(&enum_ref.name).is_none() {
                    bail!(
                        "{}.{}: arg '{}' references undefined enum '{}.{}'",
                        iface.name,
                        msg.name,
                        arg.name,
                        enum_ref.interface,
                        enum_ref.name
                    );
                }
            }
        }
    }
    Ok(())
}

/// Cross-check every `object`/`new_id` arg's `interface="..."` attribute
/// against the parsed interface set, for the same reason as
/// [`validate_enum_refs`]: the RNG only constrains the attribute's lexical
/// shape, so a typo like `interface="vitrin_grnat"` would otherwise sail
/// through -- surfacing as an opaque compile error in generated Rust and as
/// *no error at all* in the generated C header (which only embeds the name
/// in a comment).
fn validate_interface_refs(protocol: &Protocol) -> Result<()> {
    for iface in &protocol.interfaces {
        for msg in iface.requests.iter().chain(iface.events.iter()) {
            for arg in &msg.args {
                let target = match &arg.ty {
                    ArgType::Object { interface } | ArgType::NewId { interface } => interface,
                    _ => continue,
                };
                if protocol.interface(target).is_none() {
                    bail!(
                        "{}.{}: arg '{}' references unknown interface '{}'",
                        iface.name,
                        msg.name,
                        arg.name,
                        target
                    );
                }
            }
        }
    }
    Ok(())
}
