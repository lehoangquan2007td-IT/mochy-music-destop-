//! A tiny, lenient XML DOM reader — just enough for Apple-style TTML (elements, attributes,
//! text, entities, comments, CDATA, processing instructions, DOCTYPE). Whitespace text nodes
//! are preserved because TTML word boundaries live in them.

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum XNode {
    El(XElement),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct XElement {
    /// Qualified name as written (`tt:p`, `span`).
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<XNode>,
}

fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

impl XElement {
    pub fn local_name(&self) -> &str {
        local(&self.name)
    }

    /// Attribute by exact qualified name, falling back to an un-prefixed attribute of the same
    /// local name (`begin` matches `begin`).
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// Attribute by local name, ignoring any namespace prefix (`ttm:agent` for `agent`).
    pub fn attr_local(&self, local_name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| local(k) == local_name)
            .map(|(_, v)| v.as_str())
    }

    /// All descendant text, concatenated (DOM `textContent`).
    pub fn text_content(&self) -> String {
        let mut out = String::new();
        fn walk(el: &XElement, out: &mut String) {
            for c in &el.children {
                match c {
                    XNode::Text(t) => out.push_str(t),
                    XNode::El(e) => walk(e, out),
                }
            }
        }
        walk(self, &mut out);
        out
    }

    /// Every descendant element with local name `name`, in document order.
    pub fn descendants<'a>(&'a self, name: &str, out: &mut Vec<&'a XElement>) {
        for c in &self.children {
            if let XNode::El(e) = c {
                if e.local_name() == name {
                    out.push(e);
                }
                e.descendants(name, out);
            }
        }
    }
}

/// Decode the five predefined XML entities plus numeric character references.
pub(crate) fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        rest = &rest[pos..];
        let Some(semi) = rest.as_bytes().iter().take(12).position(|&c| c == b';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..semi];
        let decoded = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{00A0}'),
            _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                u32::from_str_radix(&ent[2..], 16)
                    .ok()
                    .and_then(char::from_u32)
            }
            _ if ent.starts_with('#') => ent[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Parse a document; returns its root element.
pub(crate) fn parse(src: &str) -> Result<XElement, String> {
    let mut stack: Vec<XElement> = vec![XElement {
        name: "#document".into(),
        ..Default::default()
    }];
    let mut i = 0;
    let len = src.len();
    let b = src.as_bytes();

    fn close(stack: &mut Vec<XElement>) {
        if stack.len() > 1 {
            let el = stack.pop().expect("non-empty");
            stack
                .last_mut()
                .expect("document")
                .children
                .push(XNode::El(el));
        }
    }

    while i < len {
        if b[i] != b'<' {
            let end = src[i..].find('<').map(|p| p + i).unwrap_or(len);
            let text = decode_entities(&src[i..end]);
            stack
                .last_mut()
                .expect("document")
                .children
                .push(XNode::Text(text));
            i = end;
            continue;
        }
        let rest = &src[i..];
        if rest.starts_with("<!--") {
            i = rest.find("-->").map(|p| i + p + 3).unwrap_or(len);
        } else if rest.starts_with("<![CDATA[") {
            let end = rest.find("]]>").unwrap_or(rest.len());
            let text = rest[9..end].to_string();
            stack
                .last_mut()
                .expect("document")
                .children
                .push(XNode::Text(text));
            i = (i + end + 3).min(len);
        } else if rest.starts_with("<?") {
            i = rest.find("?>").map(|p| i + p + 2).unwrap_or(len);
        } else if rest.starts_with("<!") {
            i = rest.find('>').map(|p| i + p + 1).unwrap_or(len);
        } else if rest.starts_with("</") {
            let end = rest.find('>').ok_or("unterminated end tag")?;
            let name = rest[2..end].trim();
            // lenient: close up to the matching element if it is open
            if let Some(pos) = stack.iter().rposition(|e| e.name == name)
                && pos > 0
            {
                while stack.len() > pos {
                    close(&mut stack);
                }
            }
            i += end + 1;
        } else {
            // start tag
            let mut j = i + 1;
            while j < len && !b[j].is_ascii_whitespace() && b[j] != b'>' && b[j] != b'/' {
                j += 1;
            }
            let name = src[i + 1..j].to_string();
            if name.is_empty() {
                return Err(format!("malformed tag at byte {i}"));
            }
            let mut attrs = Vec::new();
            let mut self_closing = false;
            loop {
                while j < len && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j >= len {
                    return Err("unterminated start tag".into());
                }
                if b[j] == b'>' {
                    j += 1;
                    break;
                }
                if b[j] == b'/' {
                    self_closing = true;
                    j += 1;
                    continue;
                }
                let ks = j;
                while j < len
                    && b[j] != b'='
                    && !b[j].is_ascii_whitespace()
                    && b[j] != b'>'
                    && b[j] != b'/'
                {
                    j += 1;
                }
                let key = src[ks..j].to_string();
                while j < len && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < len && b[j] == b'=' {
                    j += 1;
                    while j < len && b[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    if j < len && (b[j] == b'"' || b[j] == b'\'') {
                        let q = b[j];
                        let vs = j + 1;
                        let ve = src[vs..]
                            .find(q as char)
                            .map(|p| p + vs)
                            .ok_or("unterminated attribute")?;
                        attrs.push((key, decode_entities(&src[vs..ve])));
                        j = ve + 1;
                    } else {
                        let vs = j;
                        while j < len && !b[j].is_ascii_whitespace() && b[j] != b'>' {
                            j += 1;
                        }
                        attrs.push((key, decode_entities(&src[vs..j])));
                    }
                } else if !key.is_empty() {
                    attrs.push((key, String::new()));
                }
            }
            stack.push(XElement {
                name,
                attrs,
                children: Vec::new(),
            });
            if self_closing {
                close(&mut stack);
            }
            i = j;
        }
    }
    while stack.len() > 1 {
        close(&mut stack);
    }
    let doc = stack.pop().expect("document");
    doc.children
        .into_iter()
        .find_map(|c| match c {
            XNode::El(e) => Some(e),
            XNode::Text(_) => None,
        })
        .ok_or_else(|| "no root element".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_with_entities_and_whitespace() {
        let doc = parse(
            r#"<?xml version="1.0"?><!-- c --><tt xmlns:ttm="x"><body><p begin='1.0' ttm:agent="v1"><span>A&amp;B</span> <span>C</span><br/></p></body></tt>"#,
        )
        .unwrap();
        assert_eq!(doc.name, "tt");
        let mut ps = Vec::new();
        doc.descendants("p", &mut ps);
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].attr("begin"), Some("1.0"));
        assert_eq!(ps[0].attr_local("agent"), Some("v1"));
        assert_eq!(ps[0].text_content(), "A&B C");
        assert_eq!(ps[0].children.len(), 4);
    }
}
