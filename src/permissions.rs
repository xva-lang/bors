use axum::async_trait;
use std::collections::HashSet;
use std::time::{Duration, SystemTime};
use tokio::sync::Mutex;

use crate::github::GithubRepoName;

pub enum PermissionType {
    /// Can perform commands like r+.
    Review,
    /// Can start a try build.
    Try,
}

/// Decides if a GitHub user can perform various actions using the bot.
#[async_trait]
pub trait PermissionResolver {
    async fn has_permission(&self, username: &str, permission: PermissionType) -> bool;
}

/// For how long should the permissions be cached.
const CACHE_DURATION: Duration = Duration::from_secs(60);

/// Loads permission information from the Rust Team API.
pub struct TeamApiPermissionResolver {
    repo: GithubRepoName,
    permissions: Mutex<CachedUserPermissions>,
}

impl TeamApiPermissionResolver {
    pub async fn load(repo: GithubRepoName) -> anyhow::Result<Self> {
        let permissions = load_permissions(&repo).await?;

        Ok(Self {
            repo,
            permissions: Mutex::new(CachedUserPermissions::new(permissions)),
        })
    }

    async fn reload_permissions(&self) {
        let result = load_permissions(&self.repo).await;
        match result {
            Ok(perms) => *self.permissions.lock().await = CachedUserPermissions::new(perms),
            Err(error) => {
                tracing::error!("Cannot reload permissions for {}: {error:?}", self.repo);
            }
        }
    }
}

#[async_trait]
impl PermissionResolver for TeamApiPermissionResolver {
    async fn has_permission(&self, username: &str, permission: PermissionType) -> bool {
        if self.permissions.lock().await.is_stale() {
            self.reload_permissions().await;
        }

        self.permissions
            .lock()
            .await
            .permissions
            .has_permission(username, permission)
    }
}

pub struct UserPermissions {
    review_users: HashSet<String>,
    try_users: HashSet<String>,
}

impl UserPermissions {
    fn has_permission(&self, username: &str, permission: PermissionType) -> bool {
        match permission {
            PermissionType::Review => self.review_users.contains(username),
            PermissionType::Try => self.try_users.contains(username),
        }
    }
}

struct CachedUserPermissions {
    permissions: UserPermissions,
    created_at: SystemTime,
}
impl CachedUserPermissions {
    fn new(permissions: UserPermissions) -> Self {
        Self {
            permissions,
            created_at: SystemTime::now(),
        }
    }

    fn is_stale(&self) -> bool {
        self.created_at
            .elapsed()
            .map(|duration| duration > CACHE_DURATION)
            .unwrap_or(true)
    }
}

async fn load_permissions(repo: &GithubRepoName) -> anyhow::Result<UserPermissions> {
    tracing::info!("Reloading permissions for repository {repo}");

    let review_users = load_users_from_team_api(repo.name(), PermissionType::Review)
        .map_err(|error| anyhow::anyhow!("Cannot load review users: {error:?}"))?;

    let try_users = load_users_from_team_api(repo.name(), PermissionType::Try)
        .map_err(|error| anyhow::anyhow!("Cannot load try users: {error:?}"))?;
    Ok(UserPermissions {
        review_users,
        try_users,
    })
}

#[derive(serde::Deserialize)]
struct UserPermissionsResponse {
    github_users: HashSet<String>,
}

/// Loads users that are allowed to perform try/review from a local file
fn load_users_from_team_api(
    repository_name: &str,
    permission: PermissionType,
) -> anyhow::Result<HashSet<String>> {
    let permission = match permission {
        PermissionType::Review => "review",
        PermissionType::Try => "try",
    };

    let filename = format!(
        "{}/bors.{repository_name}.{permission}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let users =
        serde_json::from_str::<UserPermissionsResponse>(&std::fs::read_to_string(filename)?)?;

    Ok(users.github_users)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::load_users_from_team_api;

    #[test]
    fn test_load_users_from_team_api_review() {
        let users =
            load_users_from_team_api("__cargo-test", super::PermissionType::Review).unwrap();

        let mut test_case = HashSet::new();
        test_case.insert("some_user_name".to_string());
        assert_eq!(users, test_case);
    }

    #[test]
    fn test_load_users_from_team_api_try() {
        let users = load_users_from_team_api("__cargo-test", super::PermissionType::Try).unwrap();

        let mut test_case = HashSet::new();
        test_case.insert("some_user_name".to_string());
        assert_eq!(users, test_case);
    }
}
