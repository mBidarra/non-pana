//! Markdown com frontmatter YAML (`---` ... `---`).

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde::de::DeserializeOwned;

pub fn split(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix("---\n").or_else(|| text.strip_prefix("---\r\n"))?;
    let end = rest.find("\n---")?;
    let yaml = &rest[..end + 1];
    let after = &rest[end + 4..];
    let body = after.strip_prefix('\n').or_else(|| after.strip_prefix("\r\n")).unwrap_or(after);
    Some((yaml, body.strip_prefix('\n').unwrap_or(body)))
}

pub fn parse<T: DeserializeOwned>(text: &str) -> Result<(T, String)> {
    let Some((yaml, body)) = split(text) else { bail!("sem frontmatter") };
    let head = serde_yaml_ng::from_str(yaml).context("frontmatter inválido")?;
    Ok((head, body.to_string()))
}

pub fn render<T: Serialize>(head: &T, body: &str) -> Result<String> {
    let yaml = serde_yaml_ng::to_string(head)?;
    let mut out = String::from("---\n");
    out.push_str(&yaml);
    if !yaml.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("---\n\n");
    out.push_str(body.trim_start_matches('\n'));
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct H {
        id: String,
        tags: Vec<String>,
    }

    #[test]
    fn round_trip() {
        let h = H { id: "k-1".into(), tags: vec!["a".into()] };
        let s = render(&h, "Corpo\n\n- item").unwrap();
        let (h2, body): (H, String) = parse(&s).unwrap();
        assert_eq!(h, h2);
        assert_eq!(body.trim(), "Corpo\n\n- item");
    }
}
