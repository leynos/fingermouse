//! Object store backed repository for finger user data.
use std::sync::Arc;

use anyhow::Result;
use futures::FutureExt;
use futures::future::BoxFuture;
use object_store::ObjectStore;
use object_store::path::Path;
use thiserror::Error;

use crate::identity::Username;
use crate::user::{FingerProfile, ProfileError};

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("profile not found")]
    NotFound,
    #[error("profile parse error: {0}")]
    Parse(#[from] ProfileError),
    #[error("plan is not valid UTF-8")]
    InvalidPlanEncoding,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[cfg_attr(test, derive(Debug, Clone))]
pub struct UserRecord {
    pub profile: FingerProfile,
    pub plan: Option<String>,
}

pub trait UserStore: Send + Sync {
    fn load_user<'a>(
        &'a self,
        username: &'a Username,
        include_plan: bool,
    ) -> BoxFuture<'a, Result<UserRecord, RepositoryError>>;
}

#[derive(Clone)]
pub struct ObjectStoreUserStore {
    store: Arc<dyn ObjectStore>,
    profile_prefix: String,
    plan_prefix: String,
}

impl ObjectStoreUserStore {
    pub fn new(
        store: Arc<dyn ObjectStore>,
        profile_prefix: impl Into<String>,
        plan_prefix: impl Into<String>,
    ) -> Self {
        Self {
            store,
            profile_prefix: trim_slashes(profile_prefix.into()),
            plan_prefix: trim_slashes(plan_prefix.into()),
        }
    }

    fn profile_path(&self, username: &Username) -> Path {
        let file = format!("{username}.toml");
        Self::join(&self.profile_prefix, &file)
    }

    fn plan_path(&self, username: &Username) -> Path {
        let file = format!("{username}.plan");
        Self::join(&self.plan_prefix, &file)
    }

    fn join(prefix: &str, file: &str) -> Path {
        if prefix.is_empty() {
            Path::from(file.to_string())
        } else {
            Path::from(format!("{prefix}/{file}"))
        }
    }

    async fn fetch_bytes(&self, path: &Path) -> Result<Vec<u8>, object_store::Error> {
        let result = self.store.get(path).await?;
        let bytes = result.bytes().await?;
        Ok(bytes.to_vec())
    }
}

impl UserStore for ObjectStoreUserStore {
    fn load_user<'a>(
        &'a self,
        username: &'a Username,
        include_plan: bool,
    ) -> BoxFuture<'a, Result<UserRecord, RepositoryError>> {
        async move {
            let profile_path = self.profile_path(username);
            let bytes = match self.fetch_bytes(&profile_path).await {
                Ok(payload) => payload,
                Err(object_store::Error::NotFound { .. }) => {
                    return Err(RepositoryError::NotFound);
                }
                Err(err) => return Err(RepositoryError::Other(err.into())),
            };

            let profile =
                FingerProfile::parse(username.clone(), &bytes).map_err(RepositoryError::Parse)?;

            let plan = if include_plan {
                let plan_path = self.plan_path(username);
                match self.fetch_bytes(&plan_path).await {
                    Ok(content) => Some(
                        String::from_utf8(content)
                            .map_err(|_| RepositoryError::InvalidPlanEncoding)?,
                    ),
                    Err(object_store::Error::NotFound { .. }) => None,
                    Err(err) => return Err(RepositoryError::Other(err.into())),
                }
            } else {
                None
            };

            Ok(UserRecord { profile, plan })
        }
        .boxed()
    }
}

fn trim_slashes(input: impl AsRef<str>) -> String {
    input.as_ref().trim_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Username;
    use anyhow::{Result, anyhow};
    use object_store::local::LocalFileSystem;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn write_file(path: &std::path::Path, contents: &str) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, contents)?;
        Ok(())
    }

    #[tokio::test]
    async fn loads_profile_and_plan() -> Result<()> {
        let tmp = TempDir::new()?;
        let root = tmp.path();
        write_file(
            &root.join("profiles/alice.toml"),
            r#"
                username = "alice"
                full_name = "Alice Example"
            "#,
        )?;
        write_file(&root.join("plans/alice.plan"), "improve documentation\n")?;

        let store = LocalFileSystem::new_with_prefix(root)?;
        let repo = ObjectStoreUserStore::new(Arc::new(store), "profiles", "plans");
        let username = Username::parse("alice").map_err(|err| anyhow!(err))?;
        let record = repo.load_user(&username, true).await?;
        assert!(record.plan.is_some());
        let response = record.profile.render(true, record.plan.as_deref());
        let text = String::from_utf8(response.as_bytes())?;
        assert!(text.contains("Alice Example"));
        assert!(text.contains("improve documentation"));
        Ok(())
    }

    #[tokio::test]
    async fn reports_missing_user() -> Result<()> {
        let tmp = TempDir::new()?;
        let root: PathBuf = tmp.path().into();
        let store = LocalFileSystem::new_with_prefix(&root)?;
        let repo = ObjectStoreUserStore::new(Arc::new(store), "profiles", "plans");
        let username = Username::parse("bob").map_err(|err| anyhow!(err))?;
        let outcome = repo.load_user(&username, false).await;
        assert!(matches!(outcome, Err(RepositoryError::NotFound)));
        Ok(())
    }
}
