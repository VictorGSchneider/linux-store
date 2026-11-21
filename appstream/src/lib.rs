//! Streaming AppStream catalog parser. Catalogs are large (Flathub's is 48 MiB), so the XML is
//! read event by event and each `<component>` is handed to the caller as soon as it ends.
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use flate2::read::GzDecoder;
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

/// An `<icon>` entry of a component.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Icon {
    /// The `type` attribute: `cached`, `stock`, `remote` or `local`.
    pub kind: String,
    pub width: Option<u32>,
    pub value: String,
}

/// The parts of a `<component>` the backends use. Localized (`xml:lang`) names and summaries
/// are ignored.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Component {
    pub kind: String,
    pub id: String,
    pub name: Option<String>,
    pub summary: Option<String>,
    pub homepage: Option<String>,
    pub bundle: Option<String>,
    pub pkgnames: Vec<String>,
    pub categories: Vec<String>,
    pub icons: Vec<Icon>,
    /// (timestamp, version), in document order.
    pub releases: Vec<(u64, String)>,
}

enum Field {
    Id,
    Name,
    Summary,
    Homepage,
    Bundle,
    Pkgname,
    Category,
    Icon { kind: String, width: Option<u32> },
}

fn attribute(e: &BytesStart, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .map(
            |a| match a.normalized_value(quick_xml::XmlVersion::Implicit1_0) {
                Ok(value) => value.into_owned(),
                Err(_) => String::from_utf8_lossy(&a.value).into_owned(),
            },
        )
}

fn open(path: &Path) -> Result<Box<dyn Read>, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if path.extension().is_some_and(|ext| ext == "gz") {
        Ok(Box::new(GzDecoder::new(file)))
    } else {
        Ok(Box::new(file))
    }
}

/// Parses the catalog at `path` (gzip when the extension is `.gz`), calling `on_component` for
/// every `<component>` in document order. Returns the `origin` attribute of `<components>`.
pub fn parse_file(
    path: &Path,
    on_component: &mut dyn FnMut(Component),
) -> Result<Option<String>, String> {
    let mut xml = Reader::from_reader(BufReader::new(open(path)?));
    let mut buf = Vec::new();
    let mut path_stack: Vec<Vec<u8>> = Vec::new();
    let mut component: Option<Component> = None;
    // The field being read and the depth its element sits at.
    let mut field: Option<(Field, usize)> = None;
    let mut text = String::new();
    let mut origin = None;
    let mut saw_root = false;

    loop {
        let event = xml
            .read_event_into(&mut buf)
            .map_err(|e| format!("invalid XML near byte {}: {e}", xml.buffer_position()))?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let is_empty = matches!(event, Event::Empty(_));
                let name = e.name().as_ref().to_vec();
                let depth = path_stack.len() + 1;
                if depth == 1 {
                    if name != b"components" {
                        return Err("not an AppStream catalog".to_string());
                    }
                    saw_root = true;
                    origin = attribute(e, b"origin");
                }
                if depth == 2 && name == b"component" {
                    component = Some(Component {
                        kind: attribute(e, b"type").unwrap_or_default(),
                        ..Component::default()
                    });
                } else if depth == 3 && component.is_some() && !is_empty {
                    let tagged = attribute(e, b"xml:lang").is_some();
                    field = match name.as_slice() {
                        b"id" => Some(Field::Id),
                        b"name" if !tagged => Some(Field::Name),
                        b"summary" if !tagged => Some(Field::Summary),
                        b"pkgname" => Some(Field::Pkgname),
                        b"icon" => Some(Field::Icon {
                            kind: attribute(e, b"type").unwrap_or_default(),
                            width: attribute(e, b"width").and_then(|w| w.parse().ok()),
                        }),
                        b"url" if attribute(e, b"type").as_deref() == Some("homepage") => {
                            Some(Field::Homepage)
                        }
                        b"bundle" if attribute(e, b"type").as_deref() != Some("appdata") => {
                            Some(Field::Bundle)
                        }
                        _ => None,
                    }
                    .map(|f| (f, depth));
                    text.clear();
                } else if depth == 4
                    && component.is_some()
                    && !is_empty
                    && name == b"category"
                    && path_stack.get(2).is_some_and(|p| p == b"categories")
                {
                    field = Some((Field::Category, depth));
                    text.clear();
                } else if depth == 4
                    && name == b"release"
                    && path_stack.get(2).is_some_and(|p| p == b"releases")
                    && let Some(c) = component.as_mut()
                    && let Some(version) = attribute(e, b"version")
                    // A development release is a pre-release, never what the app is at.
                    && attribute(e, b"type").as_deref() != Some("development")
                {
                    let timestamp = attribute(e, b"timestamp")
                        .and_then(|t| t.parse().ok())
                        .unwrap_or(0);
                    c.releases.push((timestamp, version));
                }
                if !is_empty {
                    path_stack.push(name);
                }
            }
            Event::Text(ref t) => {
                if field.as_ref().is_some_and(|(_, d)| *d == path_stack.len()) {
                    text.push_str(&t.decode().map_err(|e| e.to_string())?);
                }
            }
            Event::CData(ref c) => {
                if field.as_ref().is_some_and(|(_, d)| *d == path_stack.len()) {
                    text.push_str(&c.decode().map_err(|e| e.to_string())?);
                }
            }
            Event::GeneralRef(ref r) => {
                if field.as_ref().is_some_and(|(_, d)| *d == path_stack.len()) {
                    if let Some(c) = r.resolve_char_ref().map_err(|e| e.to_string())? {
                        text.push(c);
                    } else {
                        match r.decode().map_err(|e| e.to_string())?.as_ref() {
                            "amp" => text.push('&'),
                            "lt" => text.push('<'),
                            "gt" => text.push('>'),
                            "quot" => text.push('"'),
                            "apos" => text.push('\''),
                            _ => {}
                        }
                    }
                }
            }
            Event::End(ref e) => {
                let depth = path_stack.len();
                if field.as_ref().is_some_and(|(_, d)| *d == depth)
                    && let (Some((f, _)), Some(c)) = (field.take(), component.as_mut())
                {
                    let value = text.trim().to_string();
                    match f {
                        Field::Id => c.id = value,
                        Field::Name => c.name = Some(value),
                        Field::Summary => c.summary = Some(value),
                        Field::Homepage => c.homepage = Some(value),
                        Field::Bundle => c.bundle = Some(value),
                        Field::Pkgname => c.pkgnames.push(value),
                        Field::Category => c.categories.push(value),
                        Field::Icon { kind, width } => c.icons.push(Icon { kind, width, value }),
                    }
                } else if depth == 2
                    && e.name().as_ref() == b"component"
                    && let Some(c) = component.take()
                {
                    on_component(c);
                }
                path_stack.pop();
            }
            Event::Eof => {
                // An empty or non-XML file must not look like a catalog with no apps.
                if !saw_root {
                    return Err("not an AppStream catalog (no <components> element)".to_string());
                }
                if !path_stack.is_empty() {
                    return Err("unexpected end of file".to_string());
                }
                return Ok(origin);
            }
            _ => {}
        }
        buf.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(xml: &str) -> (Option<String>, Vec<Component>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.xml");
        std::fs::write(&path, xml).unwrap();
        let mut found = Vec::new();
        let origin = parse_file(&path, &mut |c| found.push(c)).unwrap();
        (origin, found)
    }

    const FIREFOX: &str = r#"<?xml version="1.0"?>
<components version="1.0" origin="archlinux-arch-extra">
<component type="desktop-application">
  <id>org.mozilla.firefox</id>
  <name>Firefox</name>
  <name xml:lang="pl">Firefox PL</name>
  <summary>Browse the web</summary>
  <summary xml:lang="pl">Przegladaj</summary>
  <pkgname>firefox</pkgname>
  <pkgname>firefox-i18n</pkgname>
  <icon type="cached" width="64" height="64">firefox_64.png</icon>
  <icon type="cached" width="128" height="128">firefox_128.png</icon>
  <icon type="stock">firefox</icon>
  <categories><category>Network</category><category>WebBrowser</category></categories>
</component>
</components>"#;

    #[test]
    fn parses_pkgnames_categories_and_icons() {
        let (_, found) = parse(FIREFOX);
        let c = &found[0];
        assert_eq!(c.id, "org.mozilla.firefox");
        assert_eq!(c.pkgnames, ["firefox", "firefox-i18n"]);
        assert_eq!(c.categories, ["Network", "WebBrowser"]);
        let icon = |kind: &str, width, value: &str| Icon {
            kind: kind.into(),
            width,
            value: value.into(),
        };
        assert_eq!(
            c.icons,
            [
                icon("cached", Some(64), "firefox_64.png"),
                icon("cached", Some(128), "firefox_128.png"),
                icon("stock", None, "firefox"),
            ]
        );
    }

    #[test]
    fn ignores_localized_names() {
        let (_, found) = parse(FIREFOX);
        assert_eq!(found[0].name.as_deref(), Some("Firefox"));
        assert_eq!(found[0].summary.as_deref(), Some("Browse the web"));
    }

    #[test]
    fn reports_origin_attribute() {
        assert_eq!(parse(FIREFOX).0.as_deref(), Some("archlinux-arch-extra"));
        let (origin, found) = parse("<components version=\"0.8\"></components>");
        assert_eq!(origin, None);
        assert!(found.is_empty());
    }
}
