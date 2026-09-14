// dart-source: lib/spec-directory/options.ts
// go-source: go/spec/options.go

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SpecOptions {
    #[serde(rename = ":todo", default)]
    pub todo: Vec<String>,
    #[serde(rename = ":warning_todo", default)]
    pub warning_todo: Vec<String>,
    #[serde(rename = ":ignore_for", default)]
    pub ignore_for: Vec<String>,
    #[serde(rename = ":precision", default)]
    pub precision: i32,
}

pub fn parse_options(content: &str) -> Result<SpecOptions, String> {
    let trimmed = trim_space(content);
    if trimmed.is_empty() {
        return Ok(SpecOptions::default());
    }
    yaml_serde::from_str(trimmed).map_err(|e| format!("parsing options.yml: {}", e))
}

impl SpecOptions {
    pub fn merge(&self, child: &SpecOptions) -> SpecOptions {
        // Matches the reference harness (lib/spec-directory/options.ts:37-48),
        // which concatenates parent + child option lists.
        SpecOptions {
            precision: if child.precision != 0 {
                child.precision
            } else {
                self.precision
            },
            todo: [self.todo.clone(), child.todo.clone()].concat(),
            warning_todo: [self.warning_todo.clone(), child.warning_todo.clone()].concat(),
            ignore_for: [self.ignore_for.clone(), child.ignore_for.clone()].concat(),
        }
    }

    pub fn is_todo(&self, impl_name: &str) -> bool {
        contains_string(&self.todo, impl_name)
    }

    pub fn is_todo_any(&self, impl_names: &[&str]) -> bool {
        impl_names.iter().any(|&n| self.is_todo(n))
    }

    pub fn is_warning_todo(&self, impl_name: &str) -> bool {
        contains_string(&self.warning_todo, impl_name)
    }

    pub fn is_warning_todo_any(&self, impl_names: &[&str]) -> bool {
        impl_names.iter().any(|&n| self.is_warning_todo(n))
    }

    pub fn is_ignored(&self, impl_name: &str) -> bool {
        contains_string(&self.ignore_for, impl_name)
    }

    pub fn is_ignored_any(&self, impl_names: &[&str]) -> bool {
        impl_names.iter().any(|&n| self.is_ignored(n))
    }
}

fn contains_string(slice: &[String], s: &str) -> bool {
    slice.iter().any(|item| item.contains(s))
}

fn trim_space(s: &str) -> &str {
    s.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\r'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty() {
        let opts = parse_options("").unwrap();
        assert!(opts.todo.is_empty());
        assert!(opts.warning_todo.is_empty());
        assert!(opts.ignore_for.is_empty());
        assert_eq!(opts.precision, 0);
    }

    #[test]
    fn test_parse_whitespace_only() {
        let opts = parse_options("  \n\t  ").unwrap();
        assert!(opts.todo.is_empty());
    }

    #[test]
    fn test_parse_todo() {
        let content = ":todo:\n - sass\n";
        let opts = parse_options(content).unwrap();
        assert_eq!(opts.todo, vec!["sass"]);
    }

    #[test]
    fn test_parse_multiple_keys() {
        let content = ":todo:\n - dart-sass\n:ignore_for:\n - libsass\n:precision: 5\n";
        let opts = parse_options(content).unwrap();
        assert_eq!(opts.todo, vec!["dart-sass"]);
        assert_eq!(opts.ignore_for, vec!["libsass"]);
        assert_eq!(opts.precision, 5);
    }

    #[test]
    fn test_parse_warning_todo() {
        let content = ":warning_todo:\n - dart-sass\n";
        let opts = parse_options(content).unwrap();
        assert_eq!(opts.warning_todo, vec!["dart-sass"]);
    }

    #[test]
    fn test_merge_child_precision_overrides() {
        let parent = SpecOptions {
            precision: 3,
            ..SpecOptions::default()
        };
        let child = SpecOptions {
            precision: 5,
            ..SpecOptions::default()
        };
        let merged = parent.merge(&child);
        assert_eq!(merged.precision, 5);
    }

    #[test]
    fn test_merge_child_precision_zero_uses_parent() {
        let parent = SpecOptions {
            precision: 3,
            ..SpecOptions::default()
        };
        let child = SpecOptions {
            precision: 0,
            ..SpecOptions::default()
        };
        let merged = parent.merge(&child);
        assert_eq!(merged.precision, 3);
    }

    #[test]
    fn test_merge_child_list_concatenates() {
        let parent = SpecOptions {
            todo: vec!["a".to_string()],
            ..SpecOptions::default()
        };
        let child = SpecOptions {
            todo: vec!["b".to_string()],
            ..SpecOptions::default()
        };
        let merged = parent.merge(&child);
        assert_eq!(merged.todo, vec!["a", "b"]);
    }

    #[test]
    fn test_merge_child_empty_list_uses_parent() {
        let parent = SpecOptions {
            todo: vec!["a".to_string()],
            ..SpecOptions::default()
        };
        let child = SpecOptions::default();
        let merged = parent.merge(&child);
        assert_eq!(merged.todo, vec!["a"]);
    }

    #[test]
    fn test_is_todo_substring_match() {
        let opts = SpecOptions {
            todo: vec!["dart-sass-go".to_string()],
            ..SpecOptions::default()
        };
        assert!(opts.is_todo("dart-sass"));
    }

    #[test]
    fn test_is_todo_any() {
        let opts = SpecOptions {
            todo: vec!["dart-sass-go".to_string()],
            ..SpecOptions::default()
        };
        assert!(opts.is_todo_any(&["dart-sass", "libsass"]));
        assert!(!opts.is_todo_any(&["other"]));
    }

    #[test]
    fn test_is_ignored() {
        let opts = SpecOptions {
            ignore_for: vec!["dart-sass".to_string()],
            ..SpecOptions::default()
        };
        assert!(opts.is_ignored("dart-sass"));
        assert!(!opts.is_ignored("other"));
    }

    #[test]
    fn test_is_warning_todo() {
        let opts = SpecOptions {
            warning_todo: vec!["dart-sass".to_string()],
            ..SpecOptions::default()
        };
        assert!(opts.is_warning_todo("dart-sass"));
        assert!(!opts.is_warning_todo("other"));
    }
}
