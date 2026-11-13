//! Reading skills in the open `SKILL.md` format (REQ-SKL-001..005).
//!
//! A skill is a folder with a `SKILL.md` (a `---` header with `name` and `description`, then the instructions)
//! and, optionally, small helper files beside it. This module is pure text handling: it never opens a file,
//! never runs anything, and never follows a path. Helper files are kept as text the model can be shown as
//! reference data; they are not executed, and files whose type could be run (scripts, binaries) are skipped.

use serde::{Deserialize, Serialize};

/// Longest skill description, in characters.
pub const MAX_DESCRIPTION: usize = 300;
/// Longest `source`, in characters.
pub const MAX_SOURCE: usize = 200;
/// Most helper files in one skill.
pub const MAX_FILES: usize = 8;
/// Longest helper file, in characters.
pub const MAX_FILE_CHARS: usize = 4_000;
/// Most helper-file text in one skill, in characters.
pub const MAX_FILES_TOTAL: usize = 16_000;
/// Longest helper file path, in characters.
pub const MAX_PATH: usize = 100;
/// Deepest helper file path, in parts.
const MAX_DEPTH: usize = 4;
/// Helper files are kept only when they end in one of these: plain text that cannot be run.
pub const TEXT_EXTENSIONS: [&str; 8] = ["md", "txt", "json", "yaml", "yml", "csv", "tsv", "toml"];

/// A small text file that belongs to a skill, kept as text and never run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillFile {
    /// Where it is inside the skill folder, with `/` between parts, for example `examples/tea.md`.
    pub path: String,
    /// Its text.
    pub text: String,
}

/// A skill read from a folder, before it is checked and stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    /// From the header, or the folder's name.
    pub name: String,
    /// From the header: when to use the skill. May be empty.
    pub description: String,
    /// The text after the header.
    pub instructions: String,
    /// The `SKILL.md` it came from, as sent.
    pub source: String,
    /// Helper files that passed the checks.
    pub files: Vec<SkillFile>,
}

fn clean(s: &str) -> bool {
    !s.chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t' && c != '\r')
}

/// Why `path` cannot be a helper file path, or `None` when it can.
#[must_use]
pub fn path_problem(path: &str) -> Option<&'static str> {
    if path.is_empty() || path.chars().count() > MAX_PATH {
        return Some("the path is empty or too long");
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() > MAX_DEPTH {
        return Some("the path is nested too deeply");
    }
    if parts.iter().any(|p| {
        p.is_empty()
            || p.starts_with('.')
            || !p
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | ' '))
    }) {
        return Some(
            "the path has a part that is empty, hidden, relative or uses an odd character",
        );
    }
    let ext = path
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    if !TEXT_EXTENSIONS.contains(&ext.as_str()) {
        return Some("only plain text files are kept (md, txt, json, yaml, csv, tsv, toml)");
    }
    if path.eq_ignore_ascii_case("skill.md") || path.to_ascii_lowercase().ends_with("/skill.md") {
        return Some("a SKILL.md is a skill, not a helper file");
    }
    None
}

/// Check a whole list of helper files against the limits.
///
/// # Errors
/// A plain sentence saying what to fix.
pub fn check_files(files: &[SkillFile]) -> Result<(), String> {
    if files.len() > MAX_FILES {
        return Err(format!("A skill can have up to {MAX_FILES} helper files."));
    }
    let mut total = 0;
    for (i, f) in files.iter().enumerate() {
        if let Some(why) = path_problem(&f.path) {
            return Err(format!("Helper file \"{}\": {why}.", f.path));
        }
        if files[..i]
            .iter()
            .any(|o| o.path.eq_ignore_ascii_case(&f.path))
        {
            return Err(format!("Helper file \"{}\" is listed twice.", f.path));
        }
        let n = f.text.chars().count();
        if f.text.trim().is_empty() || n > MAX_FILE_CHARS || !clean(&f.text) {
            return Err(format!(
                "Helper file \"{}\" must be plain text of 1 to {MAX_FILE_CHARS} characters.",
                f.path
            ));
        }
        total += n;
    }
    if total > MAX_FILES_TOTAL {
        return Err(format!(
            "A skill's helper files can hold {MAX_FILES_TOTAL} characters in all."
        ));
    }
    Ok(())
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    for q in ['"', '\''] {
        if v.len() >= 2 && v.starts_with(q) && v.ends_with(q) {
            return v[1..v.len() - 1].trim().to_owned();
        }
    }
    v.to_owned()
}

/// Split a file into the `name` and `description` of its `---` header and the text after it.
/// Block scalars (`description: >-` followed by indented lines) and indented continuation lines are joined
/// with spaces. Other header keys (such as `allowed-tools` or `license`) are read past and ignored: a skill
/// cannot grant itself tools by naming them.
#[must_use]
pub fn split_header(text: &str) -> (Option<String>, Option<String>, String) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return (None, None, text.to_owned());
    }
    let mut name = None;
    let mut description = None;
    let mut key: Option<String> = None;
    let mut collected = String::new();
    let mut closed = false;
    let mut consumed = 1;
    let mut flush = |key: &mut Option<String>, collected: &mut String| {
        if let Some(k) = key.take() {
            let v = unquote(collected.trim());
            match k.as_str() {
                "name" => name = Some(v),
                "description" => description = Some(v),
                _ => {}
            }
        }
        collected.clear();
    };
    for line in text.lines().skip(1) {
        consumed += 1;
        let t = line.trim_end();
        if t == "---" || t == "..." {
            closed = true;
            break;
        }
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if indented && key.is_some() {
            if !collected.is_empty() {
                collected.push(' ');
            }
            collected.push_str(t.trim());
        } else if let Some((k, v)) = t.split_once(':')
            && !indented
            && !k.trim().is_empty()
        {
            flush(&mut key, &mut collected);
            key = Some(k.trim().to_ascii_lowercase());
            let v = v.trim();
            if !matches!(v, ">" | ">-" | ">+" | "|" | "|-" | "|+") {
                collected.push_str(v);
            }
        }
    }
    flush(&mut key, &mut collected);
    if !closed {
        // No closing line: it was not a header after all.
        return (None, None, text.to_owned());
    }
    let body = text.lines().skip(consumed).collect::<Vec<_>>().join("\n");
    (name, description, body)
}

fn folder_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

fn base(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, f)| f)
}

/// A Markdown file that is not a `README`, not a `SKILL.md` and not below any skill folder: read as a skill.
fn is_lone_md(f: &SkillFile, file: &str, roots: &[&str]) -> bool {
    !file.eq_ignore_ascii_case("skill.md")
        && file.to_ascii_lowercase().ends_with(".md")
        && !file.to_ascii_lowercase().starts_with("readme")
        // A file below a skill folder is that skill's helper file. A SKILL.md at the top makes everything
        // sent one skill.
        && !roots
            .iter()
            .any(|r| r.is_empty() || f.path.starts_with(&format!("{r}/")))
}

/// Collapse every run of whitespace to one space.
fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The helper files that belong to the skill whose `SKILL.md` is `skill`, with the reason for each skipped one
/// pushed onto `said`.
fn helper_files(
    files: &[SkillFile],
    skill: &SkillFile,
    folder: &str,
    roots: &[&str],
    said: &mut Vec<String>,
) -> Vec<SkillFile> {
    let mut kept: Vec<SkillFile> = Vec::new();
    let prefix = if folder.is_empty() {
        String::new()
    } else {
        format!("{folder}/")
    };
    for h in files {
        let Some(rel) = h.path.strip_prefix(&prefix) else {
            continue;
        };
        // A file inside another skill folder below this one belongs to that skill.
        let deeper = roots
            .iter()
            .any(|r| r.len() > folder.len() && h.path.starts_with(&format!("{r}/")));
        if std::ptr::eq(h, skill) || deeper || base(rel).eq_ignore_ascii_case("skill.md") {
            continue;
        }
        let kept_total: usize = kept.iter().map(|k| k.text.chars().count()).sum();
        let n = h.text.chars().count();
        let why = if let Some(w) = path_problem(rel) {
            Some(w.to_owned())
        } else if h.text.trim().is_empty() || n > MAX_FILE_CHARS || !clean(&h.text) {
            Some(format!(
                "it is empty, not text or over {MAX_FILE_CHARS} characters"
            ))
        } else if kept.len() >= MAX_FILES || kept_total + n > MAX_FILES_TOTAL {
            Some("the skill already has as many helper files as are kept".to_owned())
        } else {
            None
        };
        match why {
            Some(w) => said.push(format!("{}: not kept as a helper file ({w})", h.path)),
            None => kept.push(SkillFile {
                path: rel.to_owned(),
                text: h.text.clone(),
            }),
        }
    }
    kept
}

/// Read skills out of the files of one or more skill folders. A folder holding a `SKILL.md` is a skill; the
/// other small text files below it become its helper files. A lone Markdown file (not a `README`) is also
/// read as a skill. Anything that does not fit is returned in the second list with the reason, so nothing
/// disappears silently. The `path`s are only labels: nothing is read from disk.
#[must_use]
pub fn read(files: &[SkillFile]) -> (Vec<Draft>, Vec<String>) {
    let mut drafts: Vec<Draft> = Vec::new();
    let mut said: Vec<String> = Vec::new();
    // Folders that hold a SKILL.md, deepest names last, so a file goes to its nearest skill folder.
    let roots: Vec<&str> = files
        .iter()
        .filter(|f| base(&f.path).eq_ignore_ascii_case("skill.md"))
        .map(|f| folder_of(&f.path))
        .collect();
    for f in files {
        let file = base(&f.path);
        let is_skill_md = file.eq_ignore_ascii_case("skill.md");
        if !is_skill_md && !is_lone_md(f, file, &roots) {
            continue;
        }
        let where_ = f.path.chars().take(MAX_SOURCE).collect::<String>();
        if !clean(&f.text) {
            said.push(format!("{where_}: it has characters that are not text"));
            continue;
        }
        let (name, description, body) = split_header(&f.text);
        let folder = folder_of(&f.path);
        let stem = if is_skill_md {
            base(folder).to_owned()
        } else {
            file.rsplit_once('.').map_or(file, |(s, _)| s).to_owned()
        };
        let name = squash(&name.filter(|n| !n.is_empty()).unwrap_or(stem));
        let mut description = squash(&description.unwrap_or_default());
        if description.chars().count() > MAX_DESCRIPTION {
            description = description.chars().take(MAX_DESCRIPTION).collect();
            said.push(format!(
                "{where_}: the description was shortened to {MAX_DESCRIPTION} characters"
            ));
        }
        let kept = if is_skill_md {
            helper_files(files, f, folder, &roots, &mut said)
        } else {
            Vec::new()
        };
        let instructions = body.trim().to_owned();
        if name.is_empty() {
            said.push(format!("{where_}: it has no name"));
        } else if instructions.is_empty() {
            said.push(format!("{where_}: it has no instructions"));
        } else if drafts.iter().any(|d| d.name.eq_ignore_ascii_case(&name)) {
            said.push(format!("{where_}: the name {name} is already in this list"));
        } else {
            drafts.push(Draft {
                name,
                description,
                instructions,
                source: where_,
                files: kept,
            });
        }
    }
    if drafts.is_empty() && said.is_empty() {
        said.push("no SKILL.md or Markdown file was found".to_owned());
    }
    (drafts, said)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(path: &str, text: &str) -> SkillFile {
        SkillFile {
            path: path.into(),
            text: text.into(),
        }
    }

    // REQ-SKL-001
    #[test]
    fn the_header_keeps_name_and_description_apart_from_the_instructions() {
        let (n, d, body) = split_header(
            "---\nname: tea-timer\ndescription: >-\n  Use when the user wants\n  a tea timer.\nallowed-tools: Bash, Read\nlicense: MIT\n---\n# Tea\nSet a timer for four minutes.\n",
        );
        assert_eq!(n.as_deref(), Some("tea-timer"));
        assert_eq!(d.as_deref(), Some("Use when the user wants a tea timer."));
        assert_eq!(body, "# Tea\nSet a timer for four minutes.");
        // quotes, CRLF, a byte-order mark
        let (n, d, _) =
            split_header("\u{feff}---\r\nname: \"Tea\"\r\ndescription: 'x: y'\r\n---\r\nbody");
        assert_eq!((n.as_deref(), d.as_deref()), (Some("Tea"), Some("x: y")));
        // no header, or one that never closes, is all body
        assert_eq!(split_header("just text").2, "just text");
        let (n, _, body) = split_header("---\nname: x\nno end");
        assert!(n.is_none() && body.starts_with("---"));
    }

    // REQ-SKL-001, REQ-SKL-003
    #[test]
    fn a_skill_folder_becomes_a_skill_with_its_helper_files() {
        let (drafts, said) = read(&[
            f(
                "tea/SKILL.md",
                "---\nname: Tea\ndescription: Brew tea\n---\nSteep it.",
            ),
            f("tea/notes/times.md", "green 2 min"),
            f("tea/data.csv", "a,b"),
            f("tea/run.sh", "rm -rf /"),
            f("tea/bin/tool.py", "print(1)"),
            f("tea/README.md", "about"),
            f("tea/big.txt", &"x".repeat(MAX_FILE_CHARS + 1)),
            f("coffee/SKILL.md", "---\nname: Coffee\n---\nGrind it."),
            f("coffee/extra.txt", "beans"),
        ]);
        assert_eq!(drafts.len(), 2, "{said:?}");
        let tea = &drafts[0];
        assert_eq!(
            (
                tea.name.as_str(),
                tea.description.as_str(),
                tea.instructions.as_str()
            ),
            ("Tea", "Brew tea", "Steep it.")
        );
        assert_eq!(tea.source, "tea/SKILL.md");
        let paths: Vec<&str> = tea.files.iter().map(|x| x.path.as_str()).collect();
        assert_eq!(paths, ["notes/times.md", "data.csv", "README.md"]);
        assert_eq!(
            drafts[1].files.len(),
            1,
            "a file goes to its own skill only"
        );
        let said = said.join("\n");
        assert!(
            said.contains("run.sh") && said.contains("tool.py") && said.contains("big.txt"),
            "{said}"
        );
    }

    #[test]
    fn a_lone_markdown_file_a_missing_name_and_duplicates_are_handled_and_reported() {
        let (d, said) = read(&[
            f("notes/morning.md", "Say good morning."),
            f("readme.md", "ignore me"),
            f("a/SKILL.md", "---\nname: Same\n---\none"),
            f("b/SKILL.md", "---\nname: same\n---\ntwo"),
            f("c/SKILL.md", "---\nname: Empty\n---\n"),
            f("d/SKILL.md", "no header here"),
        ]);
        let names: Vec<&str> = d.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["morning", "Same", "d"], "{said:?}");
        assert_eq!(said.len(), 2, "{said:?}");
        assert_eq!(
            read(&[f("x.png", "..")]).1.len(),
            1,
            "an empty result says why"
        );
    }

    #[test]
    fn markdown_beside_a_top_level_skill_file_is_its_helper_not_a_second_skill() {
        let (drafts, said) = read(&[
            f(
                "SKILL.md",
                "---
name: tea
description: d
---
Boil water.",
            ),
            f("notes.md", "Green tea wants cooler water."),
        ]);
        assert_eq!(drafts.len(), 1, "{said:?}");
        assert_eq!(drafts[0].name, "tea");
        assert_eq!(drafts[0].files.len(), 1);
        assert_eq!(drafts[0].files[0].path, "notes.md");
    }

    #[test]
    fn a_long_description_is_shortened_and_reported() {
        let h = format!("---\nname: x\ndescription: {}\n---\nbody", "w ".repeat(400));
        let (d, said) = read(&[f("x/SKILL.md", &h)]);
        assert_eq!(d[0].description.chars().count(), MAX_DESCRIPTION);
        assert!(said[0].contains("shortened"));
    }

    // REQ-SKL-003: paths are labels, never followed.
    #[test]
    fn helper_paths_cannot_escape_or_name_something_runnable() {
        for bad in [
            "../x.md",
            "/etc/x.md",
            "a//b.md",
            "a/../b.md",
            ".hidden.md",
            "a\\b.md",
            "x.sh",
            "x.py",
            "x.exe",
            "x",
            "x.md.js",
            "a/b/c/d/e.md",
            "SKILL.md",
            "sub/skill.md",
        ] {
            assert!(path_problem(bad).is_some(), "{bad}");
        }
        for ok in ["a.md", "examples/tea notes.txt", "data/x.JSON"] {
            assert!(path_problem(ok).is_none(), "{ok}");
        }
        assert!(check_files(&[f("a.md", "x"), f("A.md", "y")]).is_err());
        assert!(check_files(&vec![f("a.md", "x"); 2]).is_err());
        let many: Vec<SkillFile> = (0..=MAX_FILES)
            .map(|i| f(&format!("{i}.md"), "x"))
            .collect();
        assert!(check_files(&many).is_err());
        let wide: Vec<SkillFile> = (0..5)
            .map(|i| f(&format!("{i}.md"), &"x".repeat(MAX_FILE_CHARS)))
            .collect();
        assert!(check_files(&wide).is_err(), "total cap");
        assert!(check_files(&[f("a.md", "bad \u{0}")]).is_err());
        assert!(check_files(&[f("a.md", "fine")]).is_ok());
    }
}
