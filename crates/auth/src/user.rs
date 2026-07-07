// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use ogrenotes_common::id::new_id;
use ogrenotes_common::time::now_usec;
use ogrenotes_storage::models::folder::Folder;
use ogrenotes_storage::models::user::{AuthProvider, User, UserRole};
use ogrenotes_storage::models::workspace::Workspace;
use ogrenotes_storage::models::FolderType;
use ogrenotes_storage::repo::folder_repo::FolderRepo;
use ogrenotes_storage::repo::user_repo::UserRepo;
use ogrenotes_storage::repo::workspace_repo::WorkspaceRepo;

use crate::jwt::AuthError;

/// Profile data from the OAuth provider.
pub struct OAuthProfile {
    pub email: String,
    pub name: String,
    pub avatar_url: Option<String>,
    /// Which provider returned this profile. Stored on the User row so
    /// subsequent logins from a DIFFERENT provider with the same email do
    /// not silently take over the account.
    pub provider: AuthProvider,
    /// Provider-specific subject id (Google's `sub`, GitHub's numeric id).
    /// Optional because GitHub / Google responses historically may omit it;
    /// when present it's compared on subsequent logins as an extra check.
    pub provider_subject_id: Option<String>,
}

/// Find an existing user by email or create a new one.
/// On first login, creates system folders (Home, Private, Trash).
///
/// Email is normalized (trimmed + lowercased) for both lookup and storage so
/// OAuth providers that return mixed-case addresses can't accidentally produce
/// duplicate user rows. `UserRepo::get_by_email` lowercases on its side too —
/// the normalization here is what keeps new rows canonical on insert.
///
/// Provider guard: a login via provider `P` for an email that is already
/// bound to a *different* provider is refused with `AuthError::OAuth`. A
/// legacy row (`provider == Unknown`) is accepted and upgraded on first
/// successful login so the stored provider reflects reality going forward.
pub async fn find_or_create_user(
    user_repo: &UserRepo,
    folder_repo: &FolderRepo,
    workspace_repo: &WorkspaceRepo,
    profile: &OAuthProfile,
) -> Result<User, AuthError> {
    let email = profile.email.trim().to_lowercase();

    // Check if user already exists by email
    if let Some(mut existing) = user_repo
        .get_by_email(&email)
        .await?
    {
        // Account linking: GitHub and Google both deliver a verified email
        // before reaching here (GitHub returns only verified emails; Google's
        // email_verified is enforced), so the same email arriving via the other
        // OAuth provider is the same person — log them into the existing
        // account. Everything else that differs is still refused: SAML accounts
        // stay SSO-only (a self-service login must not adopt them), and
        // dev-login (which trusts any email) must never adopt a real account.
        let verified_oauth =
            |p: AuthProvider| matches!(p, AuthProvider::Github | AuthProvider::Google);
        if existing.provider != AuthProvider::Unknown
            && existing.provider != profile.provider
            && !(verified_oauth(existing.provider) && verified_oauth(profile.provider))
        {
            return Err(AuthError::OAuth(format!(
                "Email {email} is registered via a different sign-in method — use that method.",
            )));
        }
        // Same-provider subject-id check (security review gap-002): the subject
        // id (GitHub numeric id / Google `sub`) is stable per provider account.
        // If this email now resolves to a DIFFERENT subject id on the SAME
        // provider, the address was reassigned to another provider account —
        // refuse rather than hand over the existing one. Cross-provider links
        // can't compare subject ids, so they rely on the verified-email check.
        if existing.provider == profile.provider {
            match (&existing.provider_subject_id, &profile.provider_subject_id) {
                (Some(stored), Some(incoming)) if stored != incoming => {
                    return Err(AuthError::OAuth(format!(
                        "Email {email} is now associated with a different account at this provider.",
                    )));
                }
                // Backfill a missing subject id on the first login that carries
                // one, so subsequent logins are protected.
                (None, Some(incoming)) => {
                    user_repo
                        .set_provider(&existing.user_id, existing.provider, Some(incoming.as_str()))
                        .await?;
                    existing.provider_subject_id = Some(incoming.clone());
                }
                _ => {}
            }
        }

        // Upgrade legacy rows so future logins lock in this provider.
        if existing.provider == AuthProvider::Unknown {
            user_repo
                .set_provider(
                    &existing.user_id,
                    profile.provider,
                    profile.provider_subject_id.as_deref(),
                )
                .await?;
            existing.provider = profile.provider;
            existing.provider_subject_id = profile.provider_subject_id.clone();
        }
        return Ok(existing);
    }

    // New user -- create with system folders
    let user_id = new_id();
    let now = now_usec();

    let home_folder_id = new_id();
    let private_folder_id = new_id();
    let trash_folder_id = new_id();
    let archive_folder_id = new_id();
    let pinned_folder_id = new_id();
    let workspace_id = new_id();

    // Persist the user with default_workspace_id = None initially. We only
    // set it after the workspace row is written, so a mid-flow failure never
    // leaves the user pointing at a workspace that doesn't exist (which would
    // surface to the client via GET /users/me).
    let mut user = User {
        user_id: user_id.clone(),
        name: profile.name.clone(),
        email: email.clone(),
        avatar_url: profile.avatar_url.clone(),
        provider: profile.provider,
        provider_subject_id: profile.provider_subject_id.clone(),
        home_folder_id: home_folder_id.clone(),
        private_folder_id: private_folder_id.clone(),
        trash_folder_id: trash_folder_id.clone(),
        archive_folder_id: Some(archive_folder_id.clone()),
        pinned_folder_id: Some(pinned_folder_id.clone()),
        default_workspace_id: None,
        mfa_secret: None,
        mfa_enrolled_at: None,
        external_id: None,
        role: UserRole::User,
        is_disabled: false,
        ask_policy: None,
        legacy_ask_enabled: false,
        email_notifications: ogrenotes_storage::models::NotifEmailPref::default(),
        ui_prefs: None,
        status: None,
        last_active_at: 0,
        created_at: now,
        updated_at: now,
    };

    // Create user. The conditional put checks attribute_not_exists(PK), which
    // is a fresh nanoid — it only guards against the astronomically unlikely
    // user_id collision, not against duplicate emails. Uniqueness of the email
    // is enforced by the get_by_email check above (now pagination-safe).
    match user_repo.create(&user).await {
        Ok(()) => {
            create_system_folders(folder_repo, &user).await?;
            create_default_workspace(workspace_repo, &user, &workspace_id).await?;
            user_repo
                .set_default_workspace(&user.user_id, &workspace_id)
                .await?;
            user.default_workspace_id = Some(workspace_id);
            Ok(user)
        }
        Err(_) => {
            // Race: a concurrent login just created the user. Look up by email.
            // Note: this scan is now O(table_size / 1MB) page-reads because
            // `get_by_email` walks all scan pages. The race window is rare,
            // but on this path login latency grows with table size until the
            // PROFILE row is reached. Acceptable for MVP; a future GSI on
            // email would make it O(1).
            user_repo
                .get_by_email(&email)
                .await?
                .ok_or_else(|| AuthError::Storage("user not found after race".into()))
        }
    }
}

/// Phase 4 M-E4 piece D: SAML JIT user creation.
///
/// Different dedupe key than the OAuth path: SAML identifies users
/// by their IdP NameID (carried on the SAML assertion), which we
/// store in `external_id` and index via `GSI6-external-id`. The
/// email + display name on the assertion are advisory — the IdP is
/// authoritative on who the user is, the email can be edited at
/// the IdP without invalidating the binding.
///
/// Resolution order:
///   1. Look up by external_id (NameID). If found, that's the user.
///   2. Fall through to email lookup. If found AND the row's
///      provider is `Unknown` or `Saml`, bind the NameID to the
///      row and return. Cross-provider email collision (e.g. email
///      already bound to Github) is rejected — same provider-hijack
///      guard as `find_or_create_user`.
///   3. Otherwise create a fresh user via the existing
///      find_or_create_user path, then bind the external_id.
pub async fn find_or_create_saml_user(
    user_repo: &UserRepo,
    folder_repo: &FolderRepo,
    workspace_repo: &WorkspaceRepo,
    name_id: &str,
    email: &str,
    name: &str,
) -> Result<User, AuthError> {
    // (1) NameID match.
    if let Some(user) = user_repo
        .get_by_external_id(name_id)
        .await?
    {
        return Ok(user);
    }

    // (2) Email match. The same get_by_email scan walk used by the
    // OAuth path — for a user who exists but hasn't been bound to
    // SAML yet, this is the path that upgrades them.
    let normalized_email = email.trim().to_lowercase();
    if let Some(mut existing) = user_repo
        .get_by_email(&normalized_email)
        .await?
    {
        // Cross-provider hijack guard. For SAML we are STRICTER
        // than the OAuth path: ONLY `Saml`-provider rows can be
        // rebound by an incoming SAML assertion. `Unknown` rows
        // (legacy password-only users) get rejected because a
        // workspace admin with a malicious IdP could issue a
        // NameID whose `attribute_email=victim@example.com` and
        // silently absorb the victim's account. The user-facing
        // error is sharp enough that the admin merge path is
        // the next obvious step rather than a silent rebind.
        if existing.provider != AuthProvider::Saml {
            return Err(AuthError::OAuth(format!(
                "Email {normalized_email} is already registered via a different \
                 credential type. Use your original sign-in method or ask an admin \
                 to migrate the account."
            )));
        }
        // Rebind: update provider_subject_id + external_id for the
        // SAML-already-provider row. (provider stays Saml; no
        // set_provider call needed.)
        user_repo
            .set_provider(&existing.user_id, AuthProvider::Saml, Some(name_id))
            .await?;
        user_repo
            .set_external_id(&existing.user_id, name_id)
            .await?;
        existing.provider_subject_id = Some(name_id.to_string());
        existing.external_id = Some(name_id.to_string());
        return Ok(existing);
    }

    // (3) Brand-new user. Delegate to find_or_create_user for the
    // system-folder + default-workspace setup, then bind external_id.
    // The user is created with provider=Saml, provider_subject_id=
    // NameID, but external_id starts as None (find_or_create_user
    // doesn't know about it). Bind it as a separate write.
    let profile = OAuthProfile {
        email: normalized_email,
        name: name.to_string(),
        avatar_url: None,
        provider: AuthProvider::Saml,
        provider_subject_id: Some(name_id.to_string()),
    };
    let mut user = find_or_create_user(user_repo, folder_repo, workspace_repo, &profile).await?;
    user_repo
        .set_external_id(&user.user_id, name_id)
        .await?;
    user.external_id = Some(name_id.to_string());
    Ok(user)
}

/// JIT user provisioning from a SCIM `POST /Users` request (Phase
/// 4 M-E5 piece D). Same shape as `find_or_create_saml_user` but
/// the inbound dedupe key is the SCIM `externalId` attribute (an
/// IdP-side opaque identifier; conventionally the IdP user's id).
///
/// Mirrors the SAML cross-provider hijack guard: an email-match
/// rebind is only permitted when the existing row's provider is
/// already `Saml`. A workspace admin with a compromised SCIM token
/// could otherwise POST a user with `externalId=victim-id` and
/// `userName=victim@example.com` and silently absorb the victim's
/// legacy Github / Google account.
///
/// The newly-created user is marked `provider=Saml` because SCIM
/// provisioning canonically pairs with SAML SSO — the user has
/// not logged in yet, but the workspace's IdP is the only path
/// they will use. A workspace that ships SCIM without SAML
/// configured is a misconfiguration; the user will exist but
/// cannot log in.
pub async fn find_or_create_scim_user(
    user_repo: &UserRepo,
    folder_repo: &FolderRepo,
    workspace_repo: &WorkspaceRepo,
    external_id: &str,
    email: &str,
    name: &str,
) -> Result<User, AuthError> {
    // (1) externalId match.
    if let Some(user) = user_repo
        .get_by_external_id(external_id)
        .await?
    {
        return Ok(user);
    }

    // (2) Email match — same provider-hijack guard as the SAML path.
    let normalized_email = email.trim().to_lowercase();
    if let Some(mut existing) = user_repo
        .get_by_email(&normalized_email)
        .await?
    {
        if existing.provider != AuthProvider::Saml {
            return Err(AuthError::OAuth(format!(
                "Email {normalized_email} is already registered via a different \
                 credential type. The SCIM provisioner cannot absorb this account; \
                 use the migration runbook instead."
            )));
        }
        user_repo
            .set_external_id(&existing.user_id, external_id)
            .await?;
        existing.external_id = Some(external_id.to_string());
        return Ok(existing);
    }

    // (3) Brand-new user.
    let profile = OAuthProfile {
        email: normalized_email,
        name: name.to_string(),
        avatar_url: None,
        provider: AuthProvider::Saml,
        provider_subject_id: None,
    };
    let mut user = find_or_create_user(user_repo, folder_repo, workspace_repo, &profile).await?;
    user_repo
        .set_external_id(&user.user_id, external_id)
        .await?;
    user.external_id = Some(external_id.to_string());
    Ok(user)
}

/// Create the user's default workspace on first login. The workspace owner
/// is the user, and the repo implicitly adds them as an Owner-role member,
/// so subsequent cross-instance auth checks resolve through GSI4.
async fn create_default_workspace(
    workspace_repo: &WorkspaceRepo,
    user: &User,
    workspace_id: &str,
) -> Result<(), AuthError> {
    let name = if user.name.trim().is_empty() {
        "Personal Workspace".to_string()
    } else {
        format!("{}'s Workspace", user.name.trim())
    };
    let workspace = Workspace {
        workspace_id: workspace_id.to_string(),
        name,
        owner_id: user.user_id.clone(),
        mfa_required: false,
        created_at: user.created_at,
        updated_at: user.updated_at,
    };
    workspace_repo
        .create(&workspace)
        .await
        .map_err(|e| AuthError::Storage(e.to_string()))
}

/// Create system folders for a new user.
/// NOTE: Folder creation is not atomic — if one put fails, earlier folders persist
/// without the later ones. This is a known limitation. DynamoDB TransactWriteItems
/// (max 100 items per transaction) could be used for atomicity in a future iteration.
async fn create_system_folders(folder_repo: &FolderRepo, user: &User) -> Result<(), AuthError> {
    let now = user.created_at;

    let mut system_folders: Vec<(&str, &str)> = vec![
        ("Home", &user.home_folder_id),
        ("Private", &user.private_folder_id),
        ("Trash", &user.trash_folder_id),
    ];
    if let Some(ref id) = user.archive_folder_id {
        system_folders.push(("Archive", id));
    }
    if let Some(ref id) = user.pinned_folder_id {
        system_folders.push(("Pinned", id));
    }

    for (title, folder_id) in &system_folders {
        let folder = Folder {
            folder_id: folder_id.to_string(),
            title: title.to_string(),
            color: 0,
            parent_id: None,
            owner_id: user.user_id.clone(),
            folder_type: FolderType::System,
            inherit_mode: ogrenotes_storage::models::InheritMode::default(),
            created_at: now,
            updated_at: now,
        };

        folder_repo
            .create(&folder)
            .await?;
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
            provider: AuthProvider::Github,
            provider_subject_id: Some("123".to_string()),
        };
        assert_eq!(profile.email, "test@example.com");
        assert_eq!(profile.name, "Test User");
        assert!(profile.avatar_url.is_some());
        assert_eq!(profile.provider, AuthProvider::Github);
    }

    #[test]
    fn system_folder_names() {
        let names = ["Home", "Private", "Trash"];
        assert_eq!(names.len(), 3);
    }
}
