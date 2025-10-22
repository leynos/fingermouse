//! Object store backed repository for finger user data.
use std::sync::Arc;

use futures::FutureExt;
use futures::future::BoxFuture;
use object_store::ObjectStore;
use object_store::path::Path;
use thiserror::Error;

use crate::identity::Username;
use crate::user::{FingerProfile, ProfileError};

#[derive(Debug, Error)]
/// Errors that may arise when retrieving user data from the backing store.
pub enum RepositoryError {
    #[error("profile not found")]
    NotFound,
    #[error("profile parse error: {0}")]
    Parse(#[from] ProfileError),
    #[error("plan is not valid UTF-8")]
    InvalidPlanEncoding,
    #[error("storage backend error: {source}")]
    Storage {
        #[source]
        source: object_store::Error,
    },
}

#[cfg_attr(test, derive(Debug, Clone))]
/// Compound user record containing the parsed profile and optional plan.
pub struct UserRecord {
    pub profile: FingerProfile,
    pub plan: Option<String>,
}

/// Abstraction over sources that provide finger user records.
pub trait UserStore: Send + Sync {
    /// Retrieve the user record for `username`, optionally including the plan
    /// file.
    fn load_user<'a>(
        &'a self,
        username: &'a Username,
        include_plan: bool,
    ) -> BoxFuture<'a, std::result::Result<UserRecord, RepositoryError>>;
}

#[derive(Clone)]
/// [`UserStore`] implementation backed by an [`ObjectStore`].
pub struct ObjectStoreUserStore {
    store: Arc<dyn ObjectStore>,
    profile_prefix: String,
    plan_prefix: String,
}

impl ObjectStoreUserStore {
    /// Create a new repository rooted at the provided object store prefixes.
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

    async fn fetch_bytes(&self, path: &Path) -> std::result::Result<Vec<u8>, object_store::Error> {
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
    ) -> BoxFuture<'a, std::result::Result<UserRecord, RepositoryError>> {
        async move {
            let profile_path = self.profile_path(username);
            let bytes = match self.fetch_bytes(&profile_path).await {
                Ok(payload) => payload,
                Err(object_store::Error::NotFound { .. }) => {
                    return Err(RepositoryError::NotFound);
                }
                Err(err) => {
                    return Err(RepositoryError::Storage { source: err });
                }
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
                    Err(err) => {
                        return Err(RepositoryError::Storage { source: err });
                    }
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
    use rstest::rstest;
    use tempfile::TempDir;

    fn write_file(path: &std::path::Path, contents: &str) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, contents)?;
        Ok(())
    }

    #[derive(Debug)]
    struct ExistingUser {
        username: &'static str,
        profile: &'static str,
        plan: Option<&'static str>,
    }

    #[derive(Debug)]
    enum Expectation {
        LoadsProfile {
            expect_plan: bool,
            expected_full_name: &'static str,
            expected_plan_fragment: Option<&'static str>,
        },
        MissingUser,
    }

    #[derive(Debug)]
    struct LoadCase {
        existing_user: Option<ExistingUser>,
        lookup: &'static str,
        include_plan: bool,
        expectation: Expectation,
    }

    impl LoadCase {
        fn profile_with_plan() -> Self {
            Self {
                existing_user: Some(ExistingUser {
                    username: "alice",
                    profile: r#"
                        username = "alice"
                        full_name = "Alice Example"
                    "#,
                    plan: Some("improve documentation\n"),
                }),
                lookup: "alice",
                include_plan: true,
                expectation: Expectation::LoadsProfile {
                    expect_plan: true,
                    expected_full_name: "Alice Example",
                    expected_plan_fragment: Some("improve documentation"),
                },
            }
        }

        fn missing_user() -> Self {
            Self {
                existing_user: None,
                lookup: "bob",
                include_plan: false,
                expectation: Expectation::MissingUser,
            }
        }
    }

    #[rstest]
    #[case::loads_profile_and_plan(LoadCase::profile_with_plan())]
    #[case::reports_missing_user(LoadCase::missing_user())]
    #[tokio::test]
    async fn load_user_behaviour(#[case] case: LoadCase) -> Result<()> {
        let tmp = TempDir::new()?;
        let root = tmp.path();

        if let Some(user) = &case.existing_user {
            // mirror production repository layout so we exercise the full IO path
            write_file(
                &root.join(format!("profiles/{}.toml", user.username)),
                user.profile,
            )?;
            if let Some(plan) = user.plan {
                write_file(&root.join(format!("plans/{}.plan", user.username)), plan)?;
            }
        }

        let store = LocalFileSystem::new_with_prefix(root)?;
        let repo = ObjectStoreUserStore::new(Arc::new(store), "profiles", "plans");
        let username = Username::parse(case.lookup).map_err(|err| anyhow!(err))?;
        let outcome = repo.load_user(&username, case.include_plan).await;

        match case.expectation {
            Expectation::LoadsProfile {
                expect_plan,
                expected_full_name,
                expected_plan_fragment,
            } => {
                let record = outcome?;
                assert_eq!(record.plan.is_some(), expect_plan);
                let response = record
                    .profile
                    .render(case.include_plan, record.plan.as_deref());
                let text = String::from_utf8(response.as_bytes())?;
                assert!(text.contains(expected_full_name));
                if let Some(snippet) = expected_plan_fragment {
                    assert!(
                        text.contains(snippet),
                        "expected rendered profile to contain plan snippet '{snippet}'"
                    );
                }
            }
            Expectation::MissingUser => {
                assert!(
                    matches!(outcome, Err(RepositoryError::NotFound)),
                    "expected missing user error"
                );
            }
        }

        Ok(())
    }
}
