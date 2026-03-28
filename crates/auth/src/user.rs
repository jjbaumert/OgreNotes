use ogrenotes_common::id::new_id;
use ogrenotes_common::time::now_usec;
use ogrenotes_storage::models::folder::Folder;
use ogrenotes_storage::models::user::User;
use ogrenotes_storage::models::FolderType;
use ogrenotes_storage::repo::folder_repo::FolderRepo;
use ogrenotes_storage::repo::user_repo::UserRepo;

use crate::jwt::AuthError;

/// Profile data from the OAuth provider.
pub struct OAuthProfile {
    pub email: String,
    pub name: String,
    pub avatar_url: Option<String>,
}

/// Find an existing user by email or create a new one.
/// On first login, creates system folders (Home, Private, Trash).
pub async fn find_or_create_user(
    user_repo: &UserRepo,
    folder_repo: &FolderRepo,
    profile: &OAuthProfile,
) -> Result<User, AuthError> {
    // Check if user already exists by email
    if let Some(existing) = user_repo
        .get_by_email(&profile.email)
        .await
        .map_err(|e| AuthError::Storage(e.to_string()))?
    {
        return Ok(existing);
    }

    // New user -- create with system folders
    let user_id = new_id();
    let now = now_usec();

    let home_folder_id = new_id();
    let private_folder_id = new_id();
    let trash_folder_id = new_id();

    let user = User {
        user_id: user_id.clone(),
        name: profile.name.clone(),
        email: profile.email.clone(),
        avatar_url: profile.avatar_url.clone(),
        home_folder_id: home_folder_id.clone(),
        private_folder_id: private_folder_id.clone(),
        trash_folder_id: trash_folder_id.clone(),
        created_at: now,
        updated_at: now,
    };

    // Create user (conditional put -- will fail if PK exists from a race)
    match user_repo.create(&user).await {
        Ok(()) => {
            create_system_folders(folder_repo, &user).await?;
            Ok(user)
        }
        Err(_) => {
            // Race condition: another request created the user first.
            // Look up by email again.
            user_repo
                .get_by_email(&profile.email)
                .await
                .map_err(|e| AuthError::Storage(e.to_string()))?
                .ok_or_else(|| AuthError::Storage("user not found after race".into()))
        }
    }
}

/// Create the three system folders for a new user.
async fn create_system_folders(folder_repo: &FolderRepo, user: &User) -> Result<(), AuthError> {
    let now = user.created_at;

    let system_folders = [
        ("Home", &user.home_folder_id),
        ("Private", &user.private_folder_id),
        ("Trash", &user.trash_folder_id),
    ];

    for (title, folder_id) in &system_folders {
        let folder = Folder {
            folder_id: folder_id.to_string(),
            title: title.to_string(),
            color: 0,
            parent_id: None,
            owner_id: user.user_id.clone(),
            folder_type: FolderType::System,
            created_at: now,
            updated_at: now,
        };

        folder_repo
            .create(&folder)
            .await
            .map_err(|e| AuthError::Storage(e.to_string()))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_profile_fields() {
        let profile = OAuthProfile {
            email: "test@example.com".to_string(),
            name: "Test User".to_string(),
            avatar_url: Some("https://example.com/avatar.png".to_string()),
        };
        assert_eq!(profile.email, "test@example.com");
        assert_eq!(profile.name, "Test User");
        assert!(profile.avatar_url.is_some());
    }

    #[test]
    fn system_folder_names() {
        let names = ["Home", "Private", "Trash"];
        assert_eq!(names.len(), 3);
    }
}
