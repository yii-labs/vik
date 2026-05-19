use std::{
  env,
  path::{Path, PathBuf},
};

/// Resolve `..` and `.` components against `from` without touching the
/// filesystem. Used instead of `std::fs::canonicalize` because workflow
/// paths must resolve identically whether or not the target exists —
/// in particular, tests use in-memory fixtures.
pub fn canonicalize_from(from: &Path, path: &Path) -> PathBuf {
  path.components().fold(from.to_path_buf(), |mut acc, comp| {
    match comp {
      std::path::Component::ParentDir => {
        acc.pop();
      },
      std::path::Component::CurDir => {},
      other => acc.push(other.as_os_str()),
    }
    acc
  })
}

/// `~` expansion only at the start. Mid-string `~` is left alone (e.g.
/// `/opt/~/literal` is a literal path). `$VAR` is *not* expanded —
/// workflow values are intentionally non-shell, so an environment
/// reference must use `env.<VAR>` in a Jinja context, not `$VAR`
/// directly in YAML.
pub fn expand_tilde(raw: &str) -> Option<PathBuf> {
  if raw == "~" {
    return dirs::home_dir();
  }

  if let Some(rest) = raw.strip_prefix("~/") {
    let mut home = dirs::home_dir()?;
    home.push(rest);
    return Some(home);
  }
  Some(PathBuf::from(raw))
}

/// `from` is the workflow file directory in production. Absolute paths
/// (after tilde expansion) pass through; relative paths get joined.
pub fn resolve_from<P: AsRef<Path>>(from: &Path, to_resolve: P) -> Option<PathBuf> {
  let expanded = expand_tilde(to_resolve.as_ref().to_str()?)?;
  if expanded.is_absolute() {
    return Some(expanded);
  }

  Some(canonicalize_from(from, &expanded))
}

const VIK_HOME_ENV_NAME: &str = "VIK_HOME";
pub fn default_home() -> PathBuf {
  env::var_os(VIK_HOME_ENV_NAME)
    .map(PathBuf::from)
    .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from("~/")))
    .join(".vik")
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_canonicalize() {
    let cwd = Path::new("/home/user");
    assert_eq!(
      canonicalize_from(cwd, Path::new("file.txt")),
      PathBuf::from("/home/user/file.txt")
    );
    assert_eq!(
      canonicalize_from(cwd, Path::new("/etc/config.yaml")),
      PathBuf::from("/etc/config.yaml")
    );
    assert_eq!(
      canonicalize_from(cwd, Path::new("../other/file.txt")),
      PathBuf::from("/home/other/file.txt")
    );
    assert_eq!(
      canonicalize_from(cwd, Path::new("./file.txt")),
      PathBuf::from("/home/user/file.txt")
    );
    assert_eq!(
      canonicalize_from(cwd, Path::new("dir/../file.txt")),
      PathBuf::from("/home/user/file.txt")
    );
  }

  #[test]
  fn tilde_alone_expands_to_home() {
    let expected = dirs::home_dir().expect("home dir available in test env");
    assert_eq!(expand_tilde("~"), Some(expected));
  }

  #[test]
  fn tilde_slash_expands_to_home_subpath() {
    let expected = dirs::home_dir().expect("home dir available in test env").join("code/vik");
    assert_eq!(expand_tilde("~/code/vik"), Some(expected));
  }

  #[test]
  fn tilde_inside_the_path_is_literal() {
    let out = expand_tilde("/opt/~/literal").expect("literal tilde returns a path");
    assert_eq!(out, PathBuf::from("/opt/~/literal"));
  }

  #[test]
  fn no_var_expansion() {
    let out = expand_tilde("$HOME/code").expect("value returned unchanged");
    assert_eq!(out, PathBuf::from("$HOME/code"));
  }

  #[test]
  fn relative_joins_onto_workflow_dir() {
    let dir = PathBuf::from("/tmp/workflows");
    let out = resolve_from(&dir, "./prompts/issues.md").expect("relative path resolves");
    assert_eq!(out, PathBuf::from("/tmp/workflows/./prompts/issues.md"));
  }

  #[test]
  fn absolute_is_untouched() {
    let dir = PathBuf::from("/tmp/workflows");
    let out = resolve_from(&dir, "/etc/vik.yml").expect("absolute path resolves");
    assert_eq!(out, PathBuf::from("/etc/vik.yml"));
  }

  #[test]
  fn tilde_expands_before_resolve() {
    let dir = PathBuf::from("/tmp/workflows");
    let expected = dirs::home_dir().expect("home dir available in test env").join(".vik");
    dbg!(&expected);
    let out = resolve_from(&dir, "~/.vik").expect("tilde expands before resolve");
    assert_eq!(out, expected);
  }

  #[test]
  fn default_home_uses_env_var() {
    let temp = tempfile::Builder::new().prefix("vik-home-").tempdir().expect("tempdir");
    let expected = temp.path().join(".vik");
    unsafe { std::env::set_var(VIK_HOME_ENV_NAME, temp.path()) };
    let out = default_home();
    assert_eq!(out, expected);
  }

  #[test]
  fn default_home_falls_back_to_user_home() {
    let expected = dirs::home_dir().expect("home dir available in test env").join(".vik");
    unsafe { std::env::remove_var(VIK_HOME_ENV_NAME) };
    let out = default_home();
    assert_eq!(out, expected);
  }
}
