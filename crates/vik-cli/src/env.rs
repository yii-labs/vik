use std::error::Error;
use std::io;

pub(crate) fn load_dotenv() -> Result<(), Box<dyn Error>> {
    match dotenvy::dotenv() {
        Ok(_) => Ok(()),
        Err(dotenvy::Error::Io(err)) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("failed to load .env: {err}").into()),
    }
}

#[cfg(test)]
fn load_dotenv_path(path: &std::path::Path) -> Result<(), Box<dyn Error>> {
    match dotenvy::from_path(path) {
        Ok(_) => Ok(()),
        Err(err) => Err(format!("failed to load {}: {err}", path.display()).into()),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use tempfile::tempdir;

    use super::*;

    fn unique_env_key(suffix: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("VIK_TEST_DOTENV_{nanos}_{suffix}")
    }

    #[test]
    fn load_dotenv_path_sets_missing_env_var() {
        let dir = tempdir().unwrap();
        let key = unique_env_key("SET");
        let env_path = dir.path().join(".env");
        fs::write(&env_path, format!("{key}=from_dotenv\n")).unwrap();

        load_dotenv_path(&env_path).unwrap();

        assert_eq!(std::env::var(key).unwrap(), "from_dotenv");
    }

    #[test]
    fn load_dotenv_path_does_not_override_existing_env_var() {
        let dir = tempdir().unwrap();
        let key = unique_env_key("PRESERVE");
        let first_path = dir.path().join(".env.first");
        let second_path = dir.path().join(".env.second");
        fs::write(&first_path, format!("{key}=first\n")).unwrap();
        fs::write(&second_path, format!("{key}=second\n")).unwrap();

        load_dotenv_path(&first_path).unwrap();
        load_dotenv_path(&second_path).unwrap();

        assert_eq!(std::env::var(key).unwrap(), "first");
    }

    #[test]
    fn load_dotenv_path_reports_parse_errors() {
        let dir = tempdir().unwrap();
        let env_path = dir.path().join(".env");
        fs::write(&env_path, "BROKEN=\"unterminated\n").unwrap();

        let err = load_dotenv_path(&env_path).unwrap_err().to_string();

        assert!(err.contains("failed to load"));
    }
}
