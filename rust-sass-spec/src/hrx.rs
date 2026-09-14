// dart-source: N/A (node-hrx module)
// go-source: go/spec/hrx.go

use std::collections::HashMap;

pub struct HrxArchive {
    pub path: String,
    pub files: HashMap<String, String>,
    pub subdirs: HashMap<String, HrxArchive>,
}

pub fn parse_hrx(content: &str) -> Result<HrxArchive, String> {
    struct Entry {
        rel_path: String,
        body: String,
    }

    let mut entries: Vec<Entry> = Vec::new();
    let mut current: Option<Entry> = None;

    for line in content.lines() {
        if line.starts_with("<===>") {
            if let Some(entry) = current.take() {
                let mut s = entry.body;
                if s.ends_with('\n') {
                    s.pop();
                }
                entries.push(Entry {
                    rel_path: entry.rel_path,
                    body: String::new(),
                });
                entries.last_mut().unwrap().body = s;
            }
            let rest = line.strip_prefix("<===>").unwrap();
            let rest = rest.trim_start_matches([' ', '\t']);
            if rest.is_empty() || rest.starts_with("===") {
                continue;
            }
            current = Some(Entry {
                rel_path: rest.to_string(),
                body: String::new(),
            });
        } else if let Some(ref mut entry) = current {
            entry.body.push_str(line);
            entry.body.push('\n');
        }
    }

    if let Some(entry) = current.take() {
        let mut s = entry.body;
        if s.ends_with('\n') {
            s.pop();
        }
        entries.push(Entry {
            rel_path: entry.rel_path,
            body: String::new(),
        });
        entries.last_mut().unwrap().body = s;
    }

    let mut archive = HrxArchive {
        path: String::new(),
        files: HashMap::new(),
        subdirs: HashMap::new(),
    };
    for e in entries {
        insert_entry(&mut archive, &e.rel_path, &e.body);
    }
    Ok(archive)
}

fn insert_entry(root: &mut HrxArchive, rel_path: &str, body: &str) {
    let parts = split_path(rel_path);
    if parts.is_empty() {
        return;
    }
    let mut current = root;
    let last_idx = parts.len() - 1;
    for (i, part) in parts.iter().enumerate() {
        if i == last_idx {
            current.files.insert(part.clone(), body.to_string());
        } else {
            if !current.subdirs.contains_key(part) {
                let sub_path = if current.path.is_empty() {
                    part.clone()
                } else {
                    format!("{}/{}", current.path, part)
                };
                current.subdirs.insert(
                    part.clone(),
                    HrxArchive {
                        path: sub_path,
                        files: HashMap::new(),
                        subdirs: HashMap::new(),
                    },
                );
            }
            current = current.subdirs.get_mut(part).unwrap();
        }
    }
}

fn split_path(p: &str) -> Vec<String> {
    let mut components: Vec<&str> = Vec::new();
    for part in p.split('/') {
        match part {
            "" | "." => continue,
            ".." => {
                components.pop();
            }
            _ => components.push(part),
        }
    }
    components.iter().map(|s| s.to_string()).collect()
}

impl HrxArchive {
    pub fn is_test_dir(&self) -> bool {
        self.files.contains_key("input.scss") || self.files.contains_key("input.sass")
    }

    pub fn input_file(&self) -> &str {
        if self.files.contains_key("input.scss") {
            "input.scss"
        } else {
            "input.sass"
        }
    }

    pub fn all_files(&self) -> HashMap<String, String> {
        let mut result = HashMap::new();
        for (name, content) in &self.files {
            result.insert(name.clone(), content.clone());
        }
        let mut names: Vec<&String> = self.subdirs.keys().collect();
        names.sort();
        for name in names {
            for (p, content) in self.subdirs[name].all_files() {
                result.insert(format!("{}/{}", name, p), content);
            }
        }
        result
    }

    pub fn list_test_dirs(&self) -> Vec<&HrxArchive> {
        let mut result = Vec::new();
        if self.is_test_dir() {
            result.push(self);
        }
        let mut names: Vec<&String> = self.subdirs.keys().collect();
        names.sort();
        for name in names {
            result.extend(self.subdirs[name].list_test_dirs());
        }
        result
    }

    pub fn get_options_yaml(&self) -> Option<&str> {
        self.files.get("options.yml").map(|s| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty_archive() {
        let archive = parse_hrx("").unwrap();
        assert!(archive.files.is_empty());
        assert!(archive.subdirs.is_empty());
    }

    #[test]
    fn test_parse_single_file() {
        let content = "<===> input.scss\na { b: c; }\n";
        let archive = parse_hrx(content).unwrap();
        assert_eq!(archive.files.get("input.scss").unwrap(), "a { b: c; }");
    }

    #[test]
    fn test_parse_multiple_files() {
        let content = "\
<===> input.scss
a { b: c; }
<===> output.css
a { b: c; }
";
        let archive = parse_hrx(content).unwrap();
        assert_eq!(archive.files.get("input.scss").unwrap(), "a { b: c; }");
        assert_eq!(archive.files.get("output.css").unwrap(), "a { b: c; }");
    }

    #[test]
    fn test_parse_nested_files() {
        let content = "\
<===> subdir/input.scss
a { b: c; }
<===> subdir/output.css
a { b: c; }
";
        let archive = parse_hrx(content).unwrap();
        let subdir = archive.subdirs.get("subdir").unwrap();
        assert_eq!(subdir.files.get("input.scss").unwrap(), "a { b: c; }");
        assert_eq!(subdir.files.get("output.css").unwrap(), "a { b: c; }");
    }

    #[test]
    fn test_parse_comment_boundary() {
        let content = "\
<===> input.scss
a { b: c; }
<===> output.css
a { b: c; }

<===>
================================================================================
<===> other/input.scss
x { y: z; }
<===> other/output.css
x { y: z; }
";
        let archive = parse_hrx(content).unwrap();
        assert_eq!(archive.files.get("input.scss").unwrap(), "a { b: c; }");
        assert_eq!(archive.files.get("output.css").unwrap(), "a { b: c; }\n");
        let other = archive.subdirs.get("other").unwrap();
        assert_eq!(other.files.get("input.scss").unwrap(), "x { y: z; }");
    }

    #[test]
    fn test_parse_trailing_newline_stripped() {
        let content = "<===> file.txt\nhello\nworld\n\n";
        let archive = parse_hrx(content).unwrap();
        assert_eq!(archive.files.get("file.txt").unwrap(), "hello\nworld\n");
    }

    #[test]
    fn test_parse_empty_file() {
        let content = "<===> empty.txt\n<===> next.txt\nbody\n";
        let archive = parse_hrx(content).unwrap();
        assert_eq!(archive.files.get("empty.txt").unwrap(), "");
        assert_eq!(archive.files.get("next.txt").unwrap(), "body");
    }

    #[test]
    fn test_parse_boundary_with_leading_spaces() {
        let content = "<===>   file.txt\ntext\n";
        let archive = parse_hrx(content).unwrap();
        assert_eq!(archive.files.get("file.txt").unwrap(), "text");
    }

    #[test]
    fn test_is_test_dir() {
        let mut archive = HrxArchive {
            path: String::new(),
            files: HashMap::new(),
            subdirs: HashMap::new(),
        };
        assert!(!archive.is_test_dir());
        archive
            .files
            .insert("input.scss".to_string(), String::new());
        assert!(archive.is_test_dir());
    }

    #[test]
    fn test_is_test_dir_sass() {
        let mut archive = HrxArchive {
            path: String::new(),
            files: HashMap::new(),
            subdirs: HashMap::new(),
        };
        archive
            .files
            .insert("input.sass".to_string(), String::new());
        assert!(archive.is_test_dir());
    }

    #[test]
    fn test_input_file_scss() {
        let mut archive = HrxArchive {
            path: String::new(),
            files: HashMap::new(),
            subdirs: HashMap::new(),
        };
        archive
            .files
            .insert("input.scss".to_string(), String::new());
        assert_eq!(archive.input_file(), "input.scss");
    }

    #[test]
    fn test_input_file_sass() {
        let mut archive = HrxArchive {
            path: String::new(),
            files: HashMap::new(),
            subdirs: HashMap::new(),
        };
        archive
            .files
            .insert("input.sass".to_string(), String::new());
        assert_eq!(archive.input_file(), "input.sass");
    }

    #[test]
    fn test_all_files_recursive() {
        let content = "\
<===> root.txt
root
<===> sub/nested.txt
nested
";
        let archive = parse_hrx(content).unwrap();
        let all = archive.all_files();
        assert_eq!(all.get("root.txt").unwrap(), "root");
        assert_eq!(all.get("sub/nested.txt").unwrap(), "nested");
    }

    #[test]
    fn test_list_test_dirs() {
        let content = "\
<===> a/input.scss
a
<===> a/output.css
a output
<===> b/input.sass
b
<===> b/output.css
b output
<===> c/README.md
not a test
";
        let archive = parse_hrx(content).unwrap();
        let dirs = archive.list_test_dirs();
        assert_eq!(dirs.len(), 2);
        assert_eq!(dirs[0].path, "a");
        assert_eq!(dirs[1].path, "b");
    }

    #[test]
    fn test_get_options_yaml() {
        let mut archive = HrxArchive {
            path: String::new(),
            files: HashMap::new(),
            subdirs: HashMap::new(),
        };
        archive
            .files
            .insert("options.yml".to_string(), ":todo:\n - sass\n".to_string());
        assert_eq!(archive.get_options_yaml().unwrap(), ":todo:\n - sass\n");
    }

    #[test]
    fn test_get_options_yaml_missing() {
        let archive = HrxArchive {
            path: String::new(),
            files: HashMap::new(),
            subdirs: HashMap::new(),
        };
        assert!(archive.get_options_yaml().is_none());
    }
}
