//! XML -> IR parsing, using `roxmltree`.
//!
//! This module assumes the input has already been validated against
//! `protocol/vitrin-v0.rng` (a separate CI step via `xmllint`, out of scope here).
//! It does not re-validate schema structure; it does perform the handful of
//! semantic checks the RNG schema cannot express (string-arg max-byte tokens,
//! enum-reference resolution), because those are exactly the facts a codegen
//! backend needs and cannot safely default or guess at.

use std::sync::LazyLock;

use anyhow::{anyhow, bail, Context, Result};
use roxmltree::{Document, Node};

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

    let mut interfaces = Vec::new();
    for iface_node in root
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "interface")
    {
        interfaces.push(parse_interface(iface_node)?);
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
                let opcode = requests.len() as u8;
                requests.push(
                    parse_message(child, opcode, &name)
                        .with_context(|| format!("interface '{name}', request #{opcode}"))?,
                );
            }
            "event" => {
                let opcode = events.len() as u8;
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

fn parse_message(node: Node, opcode: u8, own_interface: &str) -> Result<Message> {
    let name = req_attr(node, "name")?.to_string();
    let summary = description_summary(node).with_context(|| format!("message '{name}'"))?;

    let mut args = Vec::new();
    for arg_node in node
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "arg")
    {
        args.push(parse_arg(arg_node, own_interface).with_context(|| {
            format!(
                "message '{name}', arg '{}'",
                arg_node.attribute("name").unwrap_or("?")
            )
        })?);
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
            let max_bytes = MAX_BYTES_RE
                .captures(&summary)
                .ok_or_else(|| {
                    anyhow!(
                        "string arg '{name}' has no '(max N bytes)' token in its summary \
                         (summary was: {summary:?}) -- every string arg must document a bound"
                    )
                })?
                .get(1)
                .expect("regex has one capture group")
                .as_str()
                .parse::<u32>()
                .context("max-bytes token did not parse as u32")?;
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

    Ok(Arg {
        name,
        summary,
        ty,
        allow_null,
    })
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
    let bitfield = node.attribute("bitfield") == Some("true");
    let summary = description_summary(node).with_context(|| format!("enum '{name}'"))?;

    let mut entries = Vec::new();
    for entry_node in node
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "entry")
    {
        entries.push(parse_entry(entry_node).with_context(|| format!("enum '{name}'"))?);
    }
    if entries.is_empty() {
        bail!("enum '{name}' defines zero entries");
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

/// Every `protocol`, `interface`, `request`/`event`, and `enum` element carries
/// a required `<description summary="...">` child (schema-enforced). Its text
/// body is currently unused by the Rust backend beyond doc comments, but is
/// extracted for that purpose.
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
