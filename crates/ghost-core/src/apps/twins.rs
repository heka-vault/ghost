//! Two connections to the same service (2026-11-12, first sound).
//!
//! The operator connects GitHub twice, once per account, and both reach some of the same
//! repositories. The prompt said in general to "read through one of them" and the model
//! still read an issue through both connections at once in 2 of 6 runs: the line named no
//! app, so the model had to work out which apps it meant. Here it is worked out in code:
//! two apps whose service is the same (a URL's host, or the program a local app runs) and
//! whose tools are the same by the app's own names are twins, and the later one's paragraph
//! of the prompt says so, naming both. The line is Ghost's words, chosen by this code from
//! the operator's `apps.json` and the tools' names; nothing an app says decides it.

use std::collections::{BTreeSet, HashMap};

/// What [`twins`] compares of one connected app.
#[derive(Debug, Clone)]
pub struct Seen {
    /// The app's name in `apps.json` (the key of the result).
    pub name: String,
    /// How the prompt names it.
    pub shown: String,
    /// The service it reaches: `service_of`.
    pub service: String,
    /// Its tools, by the app's own names.
    pub tools: BTreeSet<String>,
}

/// The service an app reaches: a URL's host (without the port), or the file name of the
/// program a local app runs. Empty when there is neither.
pub fn service_of(url: Option<&str>, command: Option<&str>) -> String {
    if let Some(url) = url {
        let rest = url.split_once("://").map_or(url, |(_, r)| r);
        let host = rest.split(['/', '?', '#']).next().unwrap_or("");
        let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
        let host = host.split(':').next().unwrap_or("");
        return host.to_ascii_lowercase();
    }
    command
        .map(|c| {
            std::path::Path::new(c.trim())
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

/// For each app that is a twin of one before it, by `apps` order, the line its paragraph
/// ends with. The first of a set is named in every later one's line.
pub fn twins(apps: &[Seen]) -> HashMap<String, String> {
    let mut first: HashMap<(&str, &BTreeSet<String>), &Seen> = HashMap::new();
    let mut lines = HashMap::new();
    for app in apps {
        if app.service.is_empty() || app.tools.is_empty() {
            continue;
        }
        match first.get(&(app.service.as_str(), &app.tools)) {
            Some(earlier) => {
                lines.insert(app.name.clone(), line(&earlier.shown, &app.shown));
            }
            None => {
                first.insert((app.service.as_str(), &app.tools), app);
            }
        }
    }
    lines
}

/// Each app of a set of twins, with the others of its set in `apps` order: where a read it
/// cannot find is tried next.
pub fn sets(apps: &[Seen]) -> HashMap<String, Vec<String>> {
    let mut groups: Vec<((&str, &BTreeSet<String>), Vec<&str>)> = Vec::new();
    for app in apps {
        if app.service.is_empty() || app.tools.is_empty() {
            continue;
        }
        let key = (app.service.as_str(), &app.tools);
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, names)) => names.push(&app.name),
            None => groups.push((key, vec![&app.name])),
        }
    }
    let mut out = HashMap::new();
    for (_, names) in groups.iter().filter(|(_, n)| n.len() > 1) {
        for name in names {
            let others = names
                .iter()
                .filter(|n| *n != name)
                .map(|n| n.to_string())
                .collect();
            out.insert(name.to_string(), others);
        }
    }
    out
}

fn line(first: &str, this: &str) -> String {
    // Not "never both" (2026-11-12, live): the two may be accounts that see different
    // things, and an issue only the second could see was answered as not found.
    format!(
        " {this} and {first} are two connections to the same service with the same tools, \
         perhaps as different accounts that see different things. Read one thing through \
         one of them: the one that already reached it in this conversation, else {first}; \
         when it cannot find it, Ghost reads it through the other in the same call. To list \
         or count across both, ask both in one response."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(name: &str, service: &str, tools: &[&str]) -> Seen {
        Seen {
            name: name.into(),
            shown: name.into(),
            service: service.into(),
            tools: tools.iter().map(|t| t.to_string()).collect(),
        }
    }

    #[test]
    fn the_service_is_the_host_or_the_program() {
        assert_eq!(
            service_of(Some("https://api.githubcopilot.com/mcp/"), None),
            "api.githubcopilot.com"
        );
        assert_eq!(
            service_of(Some("http://user@Example.com:8080/x?y"), None),
            "example.com"
        );
        assert_eq!(
            service_of(None, Some("/usr/local/bin/ghost-mock-github")),
            "ghost-mock-github"
        );
        assert_eq!(service_of(None, None), "");
    }

    #[test]
    fn a_second_connection_to_the_same_service_names_the_first() {
        let gh = ["issue_read", "list_issues"];
        let apps = [
            seen("github", "api.githubcopilot.com", &gh),
            seen("mobbin", "mobbin.com", &["search"]),
            seen("github_quotient", "api.githubcopilot.com", &gh),
            seen("github_other", "api.githubcopilot.com", &gh),
        ];
        let t = twins(&apps);
        assert_eq!(t.len(), 2, "{t:?}");
        let line = &t["github_quotient"];
        assert!(
            line.contains("github_quotient and github are two connections"),
            "{line}"
        );
        assert!(line.contains("Ghost reads it through the other"), "{line}");
        assert!(t["github_other"].contains("else github;"));
        assert!(!t.contains_key("github"), "the first says nothing");
    }

    #[test]
    fn each_twin_knows_the_others_of_its_set() {
        let gh = ["issue_read", "list_issues"];
        let apps = [
            seen("github", "api.githubcopilot.com", &gh),
            seen("mobbin", "mobbin.com", &["search"]),
            seen("github_quotient", "api.githubcopilot.com", &gh),
        ];
        let s = sets(&apps);
        assert_eq!(s["github"], ["github_quotient"]);
        assert_eq!(s["github_quotient"], ["github"]);
        assert!(!s.contains_key("mobbin"));
    }

    #[test]
    fn other_tools_or_another_service_are_not_twins() {
        let apps = [
            seen("a", "api.githubcopilot.com", &["issue_read"]),
            seen("b", "api.githubcopilot.com", &["issue_read", "list_issues"]),
            seen("c", "gitlab.com", &["issue_read"]),
            seen("d", "", &["issue_read"]),
            seen("e", "", &["issue_read"]),
        ];
        assert!(twins(&apps).is_empty());
        assert!(sets(&apps).is_empty());
    }
}
