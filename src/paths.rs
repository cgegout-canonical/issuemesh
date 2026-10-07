use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Paths {
    pub home: PathBuf,
    pub secrets_file: PathBuf,
    pub config_file: PathBuf,
    pub db_file: PathBuf,
    pub logs_dir: PathBuf,
}

impl Paths {
    pub fn resolve(cli_data_dir: Option<&Path>, cli_config: Option<&Path>) -> Result<Self> {
        let home = if let Some(dir) = cli_data_dir {
            dir.to_path_buf()
        } else if let Ok(val) = std::env::var("ISSUEMESH_HOME") {
            PathBuf::from(val)
        } else if let Ok(val) = std::env::var("SNAP_USER_COMMON") {
            PathBuf::from(val)
        } else {
            let home_dir = dirs::home_dir().context("Could not determine user home directory")?;
            home_dir.join(".issuemesh")
        };

        let secrets_file = home.join("secrets.env");
        let config_file = if let Some(cfg) = cli_config {
            cfg.to_path_buf()
        } else {
            home.join("config.toml")
        };
        let db_file = home.join("issuemesh.db");
        let logs_dir = home.join("logs");

        Ok(Self {
            home,
            secrets_file,
            config_file,
            db_file,
            logs_dir,
        })
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true);
            builder.mode(0o700);

            if !self.home.exists() {
                builder
                    .create(&self.home)
                    .context("Failed to create home directory with mode 0700")?;
            }
            if !self.logs_dir.exists() {
                builder
                    .create(&self.logs_dir)
                    .context("Failed to create logs directory with mode 0700")?;
            }
        }

        #[cfg(not(unix))]
        {
            fs::create_dir_all(&self.home)?;
            fs::create_dir_all(&self.logs_dir)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_paths_resolve_and_ensure() {
        let tmp = tempdir().unwrap();
        let target = tmp.path().join("sub_home");
        let paths = Paths::resolve(Some(&target), None).unwrap();
        assert_eq!(paths.home, target);
        assert_eq!(paths.secrets_file, target.join("secrets.env"));
        assert_eq!(paths.config_file, target.join("config.toml"));
        assert_eq!(paths.db_file, target.join("issuemesh.db"));
        assert_eq!(paths.logs_dir, target.join("logs"));

        paths.ensure_dirs().unwrap();
        assert!(paths.home.exists());
        assert!(paths.logs_dir.exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = fs::metadata(&paths.home).unwrap();
            assert_eq!(meta.permissions().mode() & 0o777, 0o700);
        }
    }
}
