//! Count paths and anchor nodes in an SVG without a full parser. Every tracer
//! is counted by the same rules, so the numbers are comparable even if not
//! identical to what a given editor would show.
//!
//! Node = one path command (M, L, H, V, C, S, Q, T, A) including implicit
//! repeats. Z is not a node. <rect>, <circle>, <ellipse> count 4 nodes;
//! <polygon>/<polyline> count their points; <line> counts 2.

#[derive(Clone, Copy, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SvgStats {
    pub paths: usize,
    pub nodes: usize,
    pub bytes: usize,
}

pub fn stats(svg: &str) -> SvgStats {
    let mut st = SvgStats { bytes: svg.len(), ..Default::default() };
    let mut rest = svg;
    while let Some(i) = rest.find('<') {
        rest = &rest[i + 1..];
        let end = match rest.find('>') {
            Some(e) => e,
            None => break,
        };
        let tag = &rest[..end];
        let name_end = tag.find(|c: char| c.is_whitespace() || c == '/').unwrap_or(tag.len());
        let name = &tag[..name_end];
        match name {
            "path" => {
                if let Some(d) = attr(tag, "d") {
                    let n = path_nodes(d);
                    if n > 0 {
                        st.paths += 1;
                        st.nodes += n;
                    }
                }
            }
            "rect" | "circle" | "ellipse" => {
                st.paths += 1;
                st.nodes += 4;
            }
            "line" => {
                st.paths += 1;
                st.nodes += 2;
            }
            "polygon" | "polyline" => {
                if let Some(p) = attr(tag, "points") {
                    let n = p.split(|c: char| c == ',' || c.is_whitespace()).filter(|t| !t.is_empty()).count() / 2;
                    st.paths += 1;
                    st.nodes += n;
                }
            }
            _ => {}
        }
        rest = &rest[end..];
    }
    st
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut i = 0;
    while let Some(p) = tag[i..].find(name) {
        let at = i + p;
        let before_ok = at == 0 || tag.as_bytes()[at - 1].is_ascii_whitespace();
        let after = &tag[at + name.len()..];
        let after_t = after.trim_start();
        if before_ok && after_t.starts_with('=') {
            let v = after_t[1..].trim_start();
            let q = v.chars().next()?;
            if q == '"' || q == '\'' {
                let v = &v[1..];
                let e = v.find(q)?;
                return Some(&v[..e]);
            }
        }
        i = at + name.len();
    }
    None
}

fn path_nodes(d: &str) -> usize {
    let mut nodes = 0usize;
    let mut cmd: Option<char> = None;
    let mut args = 0usize; // numbers consumed for the current command
    let bytes = d.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_ascii_alphabetic() && c != 'e' && c != 'E' {
            let up = c.to_ascii_uppercase();
            if up == 'Z' {
                cmd = None;
            } else {
                cmd = Some(up);
                args = 0;
            }
            i += 1;
            continue;
        }
        if c == '-' || c == '+' || c == '.' || c.is_ascii_digit() {
            // parse one number
            let start = i;
            i += 1;
            while i < bytes.len() {
                let ch = bytes[i] as char;
                let prev = bytes[i - 1] as char;
                let is_exp_sign = (ch == '-' || ch == '+') && (prev == 'e' || prev == 'E');
                if ch.is_ascii_digit() || ch == 'e' || ch == 'E' || is_exp_sign {
                    i += 1;
                } else if ch == '.' && !d[start..i].contains('.') && !d[start..i].contains(['e', 'E']) {
                    i += 1;
                } else {
                    break;
                }
            }
            if let Some(cm) = cmd {
                let arity = match cm {
                    'M' | 'L' | 'T' => 2,
                    'H' | 'V' => 1,
                    'C' => 6,
                    'S' | 'Q' => 4,
                    'A' => 7,
                    _ => 2,
                };
                args += 1;
                if args == arity {
                    nodes += 1;
                    args = 0;
                    // implicit lineto after moveto
                    if cm == 'M' {
                        cmd = Some('L');
                    }
                }
            }
            continue;
        }
        i += 1;
    }
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn counts_explicit_and_implicit() {
        assert_eq!(path_nodes("M0 0L10 0 10 10Z"), 3);
        assert_eq!(path_nodes("M0,0c1,1 2,2 3,3 1,1 2,2 3,3z"), 3);
        assert_eq!(path_nodes("M1e-3-2.5.5.5"), 2);
        assert_eq!(path_nodes("M0 0A5 5 0 1 1 10 0H0V-5"), 4);
    }
    #[test]
    fn counts_elements() {
        let s = stats(r#"<svg><path d="M0 0L1 1Z"/><rect x="0"/><polygon points="0,0 1,0 1,1"/></svg>"#);
        assert_eq!(s.paths, 3);
        assert_eq!(s.nodes, 2 + 4 + 3);
    }
    #[test]
    fn attr_finds_right_one() {
        assert_eq!(attr(r#"path id="d" fill="red" d="M0 0""#, "d"), Some("M0 0"));
    }
}
