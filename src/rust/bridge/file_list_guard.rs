use std::path::{Path, PathBuf};

pub(super) const FILE_LIST_MAX_DEPTH: usize = 5;

pub(super) fn bounded_file_list_depth(max_depth: Option<usize>) -> usize {
    max_depth.unwrap_or(3).min(FILE_LIST_MAX_DEPTH)
}

pub(super) fn file_list_browser_roots_for_known_root(root: PathBuf) -> Vec<PathBuf> {
    if root.parent().is_none() {
        Vec::new()
    } else {
        vec![root]
    }
}

pub(super) fn sanitize_created_directory_name(name: &str) -> Result<String, &'static str> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("empty_name");
    }
    if trimmed == "." || trimmed == ".." {
        return Err("invalid_name");
    }
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains('\0') {
        return Err("path_components_not_allowed");
    }
    Ok(trimmed.to_string())
}

pub(super) fn canonical_path_is_within_allowed_roots(
    canonical_path: &Path,
    roots: &[PathBuf],
) -> bool {
    roots
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .any(|canonical_root| {
            canonical_path == canonical_root || canonical_path.starts_with(canonical_root)
        })
}

pub(super) fn canonical_file_list_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut canonical_roots = Vec::new();
    for root in roots {
        let Ok(canonical_root) = root.canonicalize() else {
            continue;
        };
        if !canonical_root.is_dir() || canonical_root.parent().is_none() {
            continue;
        }
        if !canonical_roots.contains(&canonical_root) {
            canonical_roots.push(canonical_root);
        }
    }
    canonical_roots
}

pub(super) fn file_list_browser_root_for_path(
    canonical_path: &Path,
    roots: &[PathBuf],
) -> Option<PathBuf> {
    canonical_file_list_roots(roots)
        .into_iter()
        .filter(|root| canonical_path == root || canonical_path.starts_with(root))
        .min_by_key(|root| root.components().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_list_browser_roots_do_not_expand_to_ancestors() {
        let roots = file_list_browser_roots_for_known_root(PathBuf::from("/Users/example/cunzhi"));
        assert_eq!(roots, vec![PathBuf::from("/Users/example/cunzhi")]);

        let shallow_roots = file_list_browser_roots_for_known_root(PathBuf::from("/cunzhi"));
        assert_eq!(shallow_roots, vec![PathBuf::from("/cunzhi")]);

        let filesystem_root = file_list_browser_roots_for_known_root(PathBuf::from("/"));
        assert!(filesystem_root.is_empty());
    }

    #[test]
    fn created_directory_name_rejects_path_components() {
        assert_eq!(
            sanitize_created_directory_name("  New Notes  "),
            Ok("New Notes".to_string())
        );
        assert!(sanitize_created_directory_name("").is_err());
        assert!(sanitize_created_directory_name("../escape").is_err());
        assert!(sanitize_created_directory_name("nested/folder").is_err());
        assert!(sanitize_created_directory_name(".").is_err());
        assert!(sanitize_created_directory_name("..").is_err());
    }

    #[test]
    fn canonical_roots_drop_filesystem_root_missing_paths_and_duplicates() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let root = temp_dir.path().join("home");
        std::fs::create_dir_all(&root).expect("create root");

        let roots = canonical_file_list_roots(&[
            PathBuf::from("/"),
            root.clone(),
            root.clone(),
            temp_dir.path().join("missing"),
        ]);

        assert_eq!(roots, vec![root.canonicalize().expect("canonical root")]);
    }

    #[test]
    fn browser_root_prefers_the_widest_explicit_boundary() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let home = temp_dir.path().join("home");
        let project = home.join("project");
        let source = project.join("src");
        std::fs::create_dir_all(&source).expect("create source");

        let selected = file_list_browser_root_for_path(
            &source.canonicalize().expect("canonical source"),
            &[project, home.clone()],
        );

        assert_eq!(selected, Some(home.canonicalize().expect("canonical home")));
        assert!(file_list_browser_root_for_path(Path::new("/"), &[home]).is_none());
    }
}
