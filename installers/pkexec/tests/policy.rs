//! The polkit policy and the client must agree: a typo in either makes every operation fail.
use quick_xml::Reader;
use quick_xml::events::Event;
use store_installer_pkexec::HELPER_PATH;

const POLICY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packaging/org.linuxstore.helper.policy"
);

#[derive(Default, Debug)]
struct Policy {
    action_ids: Vec<String>,
    defaults: Vec<(String, String)>,
    annotations: Vec<(String, String)>,
}

fn attribute(e: &quick_xml::events::BytesStart, key: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key.as_bytes())
        .map(|a| String::from_utf8_lossy(&a.value).into_owned())
}

fn parse() -> Policy {
    let xml = std::fs::read_to_string(POLICY).unwrap();
    let mut reader = Reader::from_str(&xml);
    let mut policy = Policy::default();
    let mut stack: Vec<String> = Vec::new();
    let mut annotation_key = None;
    loop {
        match reader.read_event().unwrap() {
            Event::Start(e) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if name == "action" {
                    policy.action_ids.extend(attribute(&e, "id"));
                }
                if name == "annotate" {
                    annotation_key = attribute(&e, "key");
                }
                stack.push(name);
            }
            Event::Text(t) => {
                let text = t.decode().unwrap().trim().to_string();
                match (stack.last().map(String::as_str), text.is_empty()) {
                    (Some(name @ ("allow_any" | "allow_inactive" | "allow_active")), false) => {
                        policy.defaults.push((name.to_string(), text));
                    }
                    (Some("annotate"), false) => {
                        policy
                            .annotations
                            .push((annotation_key.take().unwrap_or_default(), text));
                    }
                    _ => {}
                }
            }
            Event::End(_) => {
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    policy
}

#[test]
fn policy_matches_the_client_constants() {
    let policy = parse();
    assert_eq!(policy.action_ids, ["org.linuxstore.helper.run"]);
    let default = |name: &str| {
        policy
            .defaults
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(default("allow_any"), Some("auth_admin"));
    assert_eq!(default("allow_inactive"), Some("auth_admin"));
    assert_eq!(default("allow_active"), Some("auth_admin_keep"));
    assert!(
        policy.annotations.contains(&(
            "org.freedesktop.policykit.exec.path".to_string(),
            HELPER_PATH.to_string()
        )),
        "exec.path must be {HELPER_PATH}: {:?}",
        policy.annotations
    );
}

#[test]
fn policy_grants_nothing_without_authentication() {
    let policy = parse();
    assert!(
        policy
            .defaults
            .iter()
            .all(|(_, value)| value.starts_with("auth_admin")),
        "every default must require an administrator: {:?}",
        policy.defaults
    );
}
