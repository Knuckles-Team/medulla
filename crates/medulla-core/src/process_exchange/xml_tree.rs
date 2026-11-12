//! A small bounded XML element tree shared by the DMN and architecture
//! exchange readers (MEDULLA-CQ-ORCH-R702.4, R705.3): one streaming pass, no
//! DTD, no external entity, bounded size and depth.

use std::collections::BTreeMap;

use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::NsReader;

use super::ProcessExchangeError;

/// Bounds of one read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XmlLimits {
    pub max_bytes: u64,
    pub max_depth: u32,
}

impl Default for XmlLimits {
    fn default() -> Self {
        Self {
            max_bytes: medulla_types::scheduler_config::DEFAULT_PROCESS_DOCUMENT_MAX_BYTES,
            max_depth: medulla_types::scheduler_config::DEFAULT_PROCESS_DOCUMENT_MAX_DEPTH,
        }
    }
}

/// One element with its resolved namespace, attributes (raw qualified names),
/// text and children.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct XmlElement {
    pub ns: String,
    pub qname: String,
    pub local: String,
    pub attrs: BTreeMap<String, String>,
    pub children: Vec<XmlElement>,
    pub text: String,
}

impl XmlElement {
    /// The attribute whose local name is `local`.
    pub fn attr(&self, local: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(key, _)| key.as_str() == local || key.ends_with(&format!(":{local}")))
            .map(|(_, value)| value.as_str())
    }

    /// The first child with this local name.
    pub fn child(&self, local: &str) -> Option<&XmlElement> {
        self.children.iter().find(|c| c.local == local)
    }

    /// Every child with this local name.
    pub fn children_named<'a>(&'a self, local: &'a str) -> impl Iterator<Item = &'a XmlElement> {
        self.children.iter().filter(move |c| c.local == local)
    }

    /// Canonical XML of the element: sorted attributes, escaped text. The
    /// `decls` are written as `xmlns` declarations on this element.
    pub fn to_xml(&self, decls: &BTreeMap<String, String>) -> String {
        let mut out = String::new();
        self.write(&mut out, decls);
        out
    }

    fn write(&self, out: &mut String, decls: &BTreeMap<String, String>) {
        out.push('<');
        out.push_str(&self.qname);
        for (prefix, uri) in decls {
            if prefix.is_empty() {
                out.push_str(&format!(" xmlns=\"{}\"", escape(uri)));
            } else {
                out.push_str(&format!(" xmlns:{prefix}=\"{}\"", escape(uri)));
            }
        }
        for (key, value) in &self.attrs {
            out.push_str(&format!(" {key}=\"{}\"", escape(value)));
        }
        if self.children.is_empty() && self.text.is_empty() {
            out.push_str("/>");
            return;
        }
        out.push('>');
        out.push_str(&escape(&self.text));
        let none = BTreeMap::new();
        for child in &self.children {
            child.write(out, &none);
        }
        out.push_str(&format!("</{}>", self.qname));
    }
}

/// Escape the five XML special characters.
pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn position(bytes: &[u8], offset: u64) -> ProcessExchangeError {
    let end = (offset as usize).min(bytes.len());
    let before = &bytes[..end];
    let line = before.iter().filter(|&&b| b == b'\n').count() as u64 + 1;
    let column = before.iter().rev().take_while(|&&b| b != b'\n').count() as u64 + 1;
    ProcessExchangeError::Malformed { line, column }
}

fn reference_text(reference: &BytesRef<'_>) -> Result<String, ProcessExchangeError> {
    if let Some(character) = reference
        .resolve_char_ref()
        .map_err(|_| ProcessExchangeError::EntityRefused)?
    {
        return Ok(character.to_string());
    }
    let name = reference
        .decode()
        .map_err(|_| ProcessExchangeError::EntityRefused)?;
    Ok(match name.as_ref() {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "apos" => "'",
        "quot" => "\"",
        _ => return Err(ProcessExchangeError::EntityRefused),
    }
    .to_string())
}

fn open(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
    bytes: &[u8],
    offset: u64,
    decls: &mut BTreeMap<String, String>,
    first: bool,
) -> Result<XmlElement, ProcessExchangeError> {
    let (namespace, local) = reader.resolver().resolve_element(element.name());
    let ns = match namespace {
        ResolveResult::Bound(ns) => String::from_utf8_lossy(ns.as_ref()).into_owned(),
        _ => String::new(),
    };
    let mut out = XmlElement {
        ns,
        qname: String::from_utf8_lossy(element.name().as_ref()).into_owned(),
        local: String::from_utf8_lossy(local.as_ref()).into_owned(),
        ..XmlElement::default()
    };
    for attribute in element.attributes().with_checks(true) {
        let attribute = attribute.map_err(|_| position(bytes, offset))?;
        let key = String::from_utf8_lossy(attribute.key.as_ref()).into_owned();
        let value = attribute
            .normalized_value(quick_xml::XmlVersion::Explicit1_0)
            .map_err(|error| match error {
                quick_xml::Error::Escape(_) => ProcessExchangeError::EntityRefused,
                _ => position(bytes, offset),
            })?
            .into_owned();
        if key == "xmlns" {
            if first {
                decls.insert(String::new(), value);
            }
        } else if let Some(prefix) = key.strip_prefix("xmlns:") {
            if first {
                decls.insert(prefix.to_owned(), value);
            }
        } else {
            out.attrs.insert(key, value);
        }
    }
    Ok(out)
}

/// Parse one document into a tree; returns the root and its namespace
/// declarations. A DTD or an undeclared entity refuses `EntityRefused`.
pub fn parse_tree(
    bytes: &[u8],
    limits: XmlLimits,
) -> Result<(XmlElement, BTreeMap<String, String>), ProcessExchangeError> {
    if bytes.len() as u64 > limits.max_bytes {
        return Err(ProcessExchangeError::TooLarge {
            bytes: limits.max_bytes,
        });
    }
    let text =
        std::str::from_utf8(bytes).map_err(|error| position(bytes, error.valid_up_to() as u64))?;
    let mut reader = NsReader::from_str(text);
    reader.config_mut().trim_text(false);
    let mut stack: Vec<XmlElement> = Vec::new();
    let mut decls = BTreeMap::new();
    let mut root: Option<XmlElement> = None;
    loop {
        let offset = reader.buffer_position();
        let event = reader.read_event().map_err(|_| position(bytes, offset))?;
        match event {
            Event::DocType(_) => return Err(ProcessExchangeError::EntityRefused),
            Event::Start(element) => {
                if stack.len() + 1 > limits.max_depth as usize {
                    return Err(ProcessExchangeError::TooLarge {
                        bytes: limits.max_bytes,
                    });
                }
                let first = root.is_none() && stack.is_empty();
                stack.push(open(&reader, &element, bytes, offset, &mut decls, first)?);
            }
            Event::Empty(element) => {
                let first = root.is_none() && stack.is_empty();
                let done = open(&reader, &element, bytes, offset, &mut decls, first)?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(done),
                    None => root = Some(done),
                }
            }
            Event::End(_) => {
                let done = stack.pop().ok_or_else(|| position(bytes, offset))?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(done),
                    None => root = Some(done),
                }
            }
            Event::Text(t) => {
                let decoded = t.decode().map_err(|_| position(bytes, offset))?;
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&decoded);
                }
            }
            Event::CData(t) => {
                let decoded = t.decode().map_err(|_| position(bytes, offset))?;
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&decoded);
                }
            }
            Event::GeneralRef(reference) => {
                let resolved = reference_text(&reference)?;
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&resolved);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err(position(bytes, bytes.len() as u64));
    }
    let root = root.ok_or_else(|| position(bytes, bytes.len() as u64))?;
    Ok((root, decls))
}
