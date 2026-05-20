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
  if let Some(vik_home) = env::var_os(VIK_HOME_ENV_NAME).filter(|home| !home.is_empty()) {
    return PathBuf::from(vik_home);
  }

  dirs::home_dir()
    .map(|home| home.join(".vik"))
    .unwrap_or_else(|| PathBuf::from("~/.vik"))
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::process::Command;

  const DEFAULT_HOME_PROBE_PREFIX: &str = "VIK_DEFAULT_HOME_PROBE=";

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
    let out = probe_default_home(|cmd| {
      cmd.env(VIK_HOME_ENV_NAME, temp.path());
    });
    assert_eq!(out, temp.path());
  }

  #[test]
  fn default_home_falls_back_to_user_home() {
    let expected = dirs::home_dir().expect("home dir available in test env").join(".vik");
    let out = probe_default_home(|cmd| {
      cmd.env_remove(VIK_HOME_ENV_NAME);
    });
    assert_eq!(out, expected);
  }

  #[test]
  fn default_home_ignores_empty_env_var() {
    let expected = dirs::home_dir().expect("home dir available in test env").join(".vik");
    let out = probe_default_home(|cmd| {
      cmd.env(VIK_HOME_ENV_NAME, "");
    });
    assert_eq!(out, expected);
  }

  #[test]
  #[ignore = "helper for default_home tests"]
  fn default_home_probe_prints_value() {
    println!("{DEFAULT_HOME_PROBE_PREFIX}{}", default_home().display());
  }

  fn probe_default_home(configure: impl FnOnce(&mut Command)) -> PathBuf {
    let mut command = Command::new(std::env::current_exe().expect("current test exe"));
    command
      .arg("--exact")
      .arg("utils::paths::tests::default_home_probe_prints_value")
      .arg("--ignored")
      .arg("--nocapture");
    configure(&mut command);

    let output = command.output().expect("default_home probe runs");
    assert!(
      output.status.success(),
      "default_home probe failed: {}",
      String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8_lossy(&output.stdout)
      .lines()
      .find_map(|line| line.strip_prefix(DEFAULT_HOME_PROBE_PREFIX))
      .map(PathBuf::from)
      .expect("default_home probe output present")
  }
}
