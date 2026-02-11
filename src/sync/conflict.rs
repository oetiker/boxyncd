use chrono::Utc;

/// Generate a conflict copy filename.
///
/// `"report.pdf"` → `"report (Conflicted Copy 2025-06-15 10-30-45).pdf"`
/// `"Makefile"` → `"Makefile (Conflicted Copy 2025-06-15 10-30-45)"`
pub fn conflict_name(original: &str) -> String {
    let timestamp = Utc::now().format("%Y-%m-%d %H-%M-%S");
    let suffix = format!(" (Conflicted Copy {timestamp})");

    match original.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem}{suffix}.{ext}"),
        None => format!("{original}{suffix}"),
    }
}

/// Given a full relative path, rename just the filename portion.
///
/// `"docs/report.pdf"` → `"docs/report (Conflicted Copy ...).pdf"`
pub fn conflict_path(relative_path: &str) -> String {
    match relative_path.rsplit_once('/') {
        Some((dir, name)) => {
            let new_name = conflict_name(name);
            format!("{dir}/{new_name}")
        }
        None => conflict_name(relative_path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conflict_name_with_extension() {
        let name = conflict_name("report.pdf");
        assert!(name.starts_with("report (Conflicted Copy "));
        assert!(name.ends_with(").pdf"));
    }

    #[test]
    fn test_conflict_name_no_extension() {
        let name = conflict_name("Makefile");
        assert!(name.starts_with("Makefile (Conflicted Copy "));
        assert!(name.ends_with(')'));
        assert!(!name.contains('.'));
    }

    #[test]
    fn test_conflict_path_nested() {
        let path = conflict_path("docs/sub/report.pdf");
        assert!(path.starts_with("docs/sub/report (Conflicted Copy "));
        assert!(path.ends_with(").pdf"));
    }

    #[test]
    fn test_conflict_path_root() {
        let path = conflict_path("readme.txt");
        assert!(path.starts_with("readme (Conflicted Copy "));
        assert!(path.ends_with(").txt"));
    }
}
