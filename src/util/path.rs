use std::path::Path;

use anyhow::{Context, Result};

/// Compute the relative path from `base` to `full`.
/// Both paths should be absolute. Returns a forward-slash separated string
/// suitable for use as a platform-independent sync key.
pub fn relative_path(base: &Path, full: &Path) -> Result<String> {
    let rel = full
        .strip_prefix(base)
        .with_context(|| format!("{} is not under {}", full.display(), base.display()))?;

    // Normalize to forward slashes (already the case on Linux, but be explicit)
    let s = rel.to_string_lossy().replace('\\', "/");
    Ok(s)
}

/// Check whether a relative path matches any of the exclude glob patterns.
pub fn matches_exclude(relative: &str, patterns: &[String]) -> bool {
    for pattern in patterns {
        if glob_match(pattern, relative) {
            return true;
        }
    }
    false
}

/// Simple glob matching supporting `*` (single segment) and `**` (any depth).
/// This matches against the relative path using forward slashes.
fn glob_match(pattern: &str, path: &str) -> bool {
    // Handle ** patterns (match any number of path segments)
    if pattern.contains("**") {
        let parts: Vec<&str> = pattern.split("**").collect();
        if parts.len() == 2 {
            let prefix = parts[0].trim_end_matches('/');
            let suffix = parts[1].trim_start_matches('/');

            if prefix.is_empty() {
                // Pattern like `**/foo` — suffix can appear anywhere
                let suffix_ok = suffix.is_empty()
                    || path.split('/').any(|segment| simple_glob(suffix, segment));
                return suffix_ok;
            }

            // Pattern like `dir/**` or `dir/**/suffix` — prefix can match
            // any path segment or sequence. Check if any segment matches
            // the prefix, and everything after it satisfies the suffix.
            let segments: Vec<&str> = path.split('/').collect();
            for i in 0..segments.len() {
                if simple_glob(prefix, segments[i]) {
                    // Everything after this segment must match suffix
                    let rest = segments[i + 1..].join("/");
                    let suffix_ok = suffix.is_empty()
                        || simple_glob(suffix, &rest)
                        || rest.split('/').any(|seg| simple_glob(suffix, seg));
                    if suffix_ok {
                        return true;
                    }
                }
            }
            return false;
        }
    }

    // For patterns without path separators, match against the filename only
    if !pattern.contains('/') {
        let filename = path.rsplit('/').next().unwrap_or(path);
        return simple_glob(pattern, filename);
    }

    // Otherwise match the full path
    simple_glob(pattern, path)
}

/// Match a simple glob pattern (only `*` wildcard, no `**`) against a string.
fn simple_glob(pattern: &str, text: &str) -> bool {
    let mut pi = pattern.chars().peekable();
    let mut ti = text.chars().peekable();

    while pi.peek().is_some() || ti.peek().is_some() {
        match pi.peek() {
            Some('*') => {
                pi.next();
                // * matches zero or more characters
                if pi.peek().is_none() {
                    return true; // trailing * matches everything
                }
                // Try matching the rest of the pattern at every position
                let remaining_pattern: String = pi.clone().collect();
                let remaining_text: String = ti.clone().collect();
                for (i, _) in remaining_text.char_indices() {
                    if simple_glob(&remaining_pattern, &remaining_text[i..]) {
                        return true;
                    }
                }
                // Also try matching at the very end (empty string)
                if simple_glob(&remaining_pattern, "") {
                    return true;
                }
                return false;
            }
            Some('?') => {
                pi.next();
                if ti.next().is_none() {
                    return false;
                }
            }
            Some(&pc) => {
                pi.next();
                match ti.next() {
                    Some(tc) if tc == pc => {}
                    _ => return false,
                }
            }
            None => return false,
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_relative_path() {
        let base = Path::new("/home/user/sync");
        let full = Path::new("/home/user/sync/docs/report.pdf");
        assert_eq!(relative_path(base, full).unwrap(), "docs/report.pdf");
    }

    #[test]
    fn test_relative_path_root() {
        let base = Path::new("/home/user/sync");
        let full = Path::new("/home/user/sync/file.txt");
        assert_eq!(relative_path(base, full).unwrap(), "file.txt");
    }

    #[test]
    fn test_exclude_glob_star() {
        assert!(matches_exclude("foo.tmp", &["*.tmp".into()]));
        assert!(!matches_exclude("foo.txt", &["*.tmp".into()]));
    }

    #[test]
    fn test_exclude_glob_doublestar() {
        assert!(matches_exclude(
            "deep/nested/dir/.git/config",
            &[".git/**".into()]
        ));
        assert!(matches_exclude(
            "node_modules/foo/bar.js",
            &["node_modules/**".into()]
        ));
        assert!(!matches_exclude("src/main.rs", &["node_modules/**".into()]));
    }

    #[test]
    fn test_exclude_filename_only() {
        assert!(matches_exclude(
            "some/path/thumbs.db",
            &["thumbs.db".into()]
        ));
        assert!(!matches_exclude(
            "some/path/readme.md",
            &["thumbs.db".into()]
        ));
    }

    #[test]
    fn test_glob_with_combining_unicode() {
        // ä as a + combining umlaut (U+0308) — multi-byte char boundary
        let name = "Solidarita\u{308}tsanlass_24.2.2026.jpg";
        assert!(matches_exclude(name, &["*.jpg".into()]));
        assert!(!matches_exclude(name, &["*.png".into()]));
    }
}
