// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use aws_sdk_dynamodb::types::AttributeValue;
use std::collections::HashMap;

use crate::dynamo::DynamoClient;
use crate::models::user::{AskPolicy, EncryptedString, User, UserRole};
use crate::repo::{RepoError, get_s, get_n};

/// Repository for user operations.
pub struct UserRepo {
    db: DynamoClient,
}

impl UserRepo {
    pub fn new(db: DynamoClient) -> Self {
        Self { db }
    }

    /// Create a new user record.
    pub async fn create(&self, user: &User) -> Result<(), RepoError> {
        let mut item = user_to_item(user);
        item.insert("PK".to_string(), AttributeValue::S(user.pk()));
        item.insert("SK".to_string(), AttributeValue::S(User::sk().to_string()));

        self.db
            .put_item_conditional(item, "attribute_not_exists(PK)")
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        // Email→user pointer so `get_by_email` is a `GetItem`, not a
        // full-table `Scan` (#36). Best-effort: the PROFILE row above is the
        // source of truth and `get_by_email` falls back to a scan on a
        // pointer miss, so a failed pointer here must not fail create
        // (the #49 lesson). Email is immutable — SCIM forbids changing
        // userName — so the pointer never needs updating after create.
        if let Err(e) = self.put_email_pointer(&user.email, &user.user_id).await {
            tracing::warn!(
                user_id = %user.user_id,
                error = %e,
                "failed to write email pointer on create; get_by_email falls \
                 back to a scan until backfill_email_pointers runs"
            );
        }
        Ok(())
    }

    /// Write (idempotent) the `EMAIL#<lowercased> → user_id` pointer (#36).
    async fn put_email_pointer(&self, email: &str, user_id: &str) -> Result<(), RepoError> {
        let email_lc = email.trim().to_lowercase();
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(email_pointer_pk(&email_lc)));
        item.insert("SK".to_string(), AttributeValue::S(EMAIL_POINTER_SK.to_string()));
        item.insert("user_id".to_string(), AttributeValue::S(user_id.to_string()));
        self.db
            .put_item(item)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Get a user by ID.
    pub async fn get_by_id(&self, user_id: &str) -> Result<Option<User>, RepoError> {
        let pk = format!("USER#{user_id}");
        let item = self
            .db
            .get_item(&pk, User::sk())
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        match item {
            Some(item) => Ok(Some(user_from_item(&item)?)),
            None => Ok(None),
        }
    }

    /// Batch-fetch users by id in one chunked `BatchGetItem` round-trip
    /// instead of N sequential `get_by_id` calls (#38). Returns a map keyed
    /// by `user_id`; an id whose PROFILE row is missing is simply absent
    /// (same as `get_by_id` → `None`). Duplicate ids are de-duplicated.
    /// Chunks to the 100-key `BatchGetItem` limit and retries any
    /// `UnprocessedKeys` DynamoDB returns under load.
    ///
    /// This is a latency win (fewer round-trips), not a cost win — N
    /// cross-partition single-item gets bill ~the same RCU either way.
    pub async fn get_by_ids(
        &self,
        user_ids: &[String],
    ) -> Result<HashMap<String, User>, RepoError> {
        use aws_sdk_dynamodb::types::KeysAndAttributes;

        let mut seen = std::collections::HashSet::new();
        let unique: Vec<&String> = user_ids
            .iter()
            .filter(|id| seen.insert((*id).as_str()))
            .collect();

        let table = self.db.table_name().to_string();
        let mut out: HashMap<String, User> = HashMap::with_capacity(unique.len());

        for chunk in unique.chunks(100) {
            let mut kaa = KeysAndAttributes::builder();
            for id in chunk {
                let mut key = HashMap::new();
                key.insert("PK".to_string(), AttributeValue::S(format!("USER#{id}")));
                key.insert("SK".to_string(), AttributeValue::S(User::sk().to_string()));
                kaa = kaa.keys(key);
            }
            let kaa = kaa
                .build()
                .map_err(|e| RepoError::Dynamo(format!("batch_get keys: {e}")))?;

            let mut request_items: HashMap<String, KeysAndAttributes> = HashMap::new();
            request_items.insert(table.clone(), kaa);

            // Retry UnprocessedKeys (DynamoDB throttles a batch by omitting
            // keys, not erroring). Bounded so a persistent throttle can't spin.
            let mut attempt = 0u32;
            loop {
                let resp = self
                    .db
                    .inner()
                    .batch_get_item()
                    .set_request_items(Some(request_items))
                    .send()
                    .await
                    .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;

                if let Some(mut responses) = resp.responses {
                    if let Some(items) = responses.remove(&table) {
                        for item in items {
                            let user = user_from_item(&item)?;
                            out.insert(user.user_id.clone(), user);
                        }
                    }
                }

                match resp.unprocessed_keys {
                    Some(unprocessed) if !unprocessed.is_empty() && attempt < 5 => {
                        attempt += 1;
                        request_items = unprocessed;
                    }
                    _ => break,
                }
            }
        }
        Ok(out)
    }

    /// Get a user by email (scans USER#/PROFILE items).
    ///
    /// A DynamoDB scan returns at most 1MB per call; matching items past that
    /// boundary are only reachable via `LastEvaluatedKey` continuation. The
    /// loop here walks every page until either a match is found or the scan
    /// is exhausted. Without it, a live table >1MB can silently miss an
    /// existing user and cause `find_or_create_user` to insert a duplicate.
    /// Email is lowercased to match the canonical form written by
    /// `find_or_create_user`.
    pub async fn get_by_email(&self, email: &str) -> Result<Option<User>, RepoError> {
        let email_lc = email.trim().to_lowercase();

        // Fast path (#36): resolve the `EMAIL#<lc>` pointer with a single
        // GetItem, then load the user by id. Falls through to the legacy
        // scan when the pointer is absent (users created before #36, until
        // `backfill_email_pointers` runs) or stale (points at a since-deleted
        // user).
        if let Some(item) = self
            .db
            .get_item(&email_pointer_pk(&email_lc), EMAIL_POINTER_SK)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?
        {
            if let Ok(user_id) = get_s(&item, "user_id") {
                if let Some(user) = self.get_by_id(&user_id).await? {
                    return Ok(Some(user));
                }
            }
        }

        self.get_by_email_scan(&email_lc).await
    }

    /// Legacy full-table `Scan` for `get_by_email`, retained as the pointer
    /// fallback (#36). `email_lc` must already be trimmed + lowercased.
    async fn get_by_email_scan(&self, email_lc: &str) -> Result<Option<User>, RepoError> {
        let mut last_key: Option<HashMap<String, AttributeValue>> = None;
        loop {
            let mut builder = self
                .db
                .inner()
                .scan()
                .table_name(self.db.table_name())
                .filter_expression("SK = :sk AND email = :email")
                .expression_attribute_values(":sk", AttributeValue::S("PROFILE".to_string()))
                .expression_attribute_values(":email", AttributeValue::S(email_lc.to_string()));
            if let Some(key) = last_key.take() {
                builder = builder.set_exclusive_start_key(Some(key));
            }
            let result = builder
                .send()
                .await
                .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;

            if let Some(items) = result.items {
                if let Some(item) = items.into_iter().next() {
                    return Ok(Some(user_from_item(&item)?));
                }
            }

            match result.last_evaluated_key {
                Some(key) => last_key = Some(key),
                None => return Ok(None),
            }
        }
    }

    /// One-time migration (#36): write an `EMAIL#<lc> → user_id` pointer for
    /// every existing PROFILE row so `get_by_email` stops scanning.
    /// Idempotent (pointers are put-overwrites) — safe to re-run. Returns
    /// `(profiles_scanned, pointers_written)`.
    pub async fn backfill_email_pointers(&self) -> Result<(usize, usize), RepoError> {
        let mut scanned = 0usize;
        let mut written = 0usize;
        let mut start_key: Option<HashMap<String, AttributeValue>> = None;
        loop {
            let mut builder = self
                .db
                .inner()
                .scan()
                .table_name(self.db.table_name())
                .filter_expression("SK = :sk")
                .expression_attribute_values(":sk", AttributeValue::S("PROFILE".to_string()));
            if let Some(start) = start_key.take() {
                builder = builder.set_exclusive_start_key(Some(start));
            }
            let result = builder
                .send()
                .await
                .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;

            for item in result.items.unwrap_or_default() {
                let (Ok(email), Ok(user_id)) = (get_s(&item, "email"), get_s(&item, "user_id"))
                else {
                    continue;
                };
                scanned += 1;
                self.put_email_pointer(&email, &user_id).await?;
                written += 1;
            }

            match result.last_evaluated_key {
                Some(key) => start_key = Some(key),
                None => break,
            }
        }
        Ok((scanned, written))
    }

    /// Look up a user by their SCIM `externalId` / SAML NameID via
    /// the sparse `GSI6-external-id` index (Phase 4 M-E4 piece D
    /// consumer; future M-E5 SCIM consumer will use the same
    /// helper). Returns `None` when no row has this external_id —
    /// callers (SAML JIT, SCIM provisioning) typically interpret
    /// that as "fall through to create-new-user."
    pub async fn get_by_external_id(
        &self,
        external_id: &str,
    ) -> Result<Option<User>, RepoError> {
        let items = self
            .db
            .query_index(
                "GSI6-external-id",
                "external_id_gsi",
                external_id,
                None,
                None,
                false,
                None,
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        // GSI is hash-only and sparse on a unique attribute; at
        // most one row can match. Take the first PROFILE row we
        // see (defensive against any future second index row that
        // happens to share an external_id).
        for item in items {
            let sk = item.get("SK").and_then(|v| v.as_s().ok());
            if sk.map(|s| s.as_str()) == Some("PROFILE") {
                return Ok(Some(user_from_item(&item)?));
            }
        }
        Ok(None)
    }

    /// Search users by email or name substring (case-insensitive).
    /// Returns up to `MAX_MATCHES` matches.
    ///
    /// This is a `Scan` with a post-read `contains()` filter — DynamoDB
    /// can't index a substring. It stops early once enough matches are
    /// collected, but a query that matches *few or no* users would
    /// otherwise paginate the whole table (the filter discards most
    /// items server-side), so cost scaled with the entire user set, not
    /// the result (issue #35). A **scanned-items budget** bounds that:
    /// once `MAX_SCANNED_ITEMS` rows have been examined we stop and
    /// return what we have. The budget comfortably exceeds expected
    /// per-deployment user counts (typical workspaces are <500 members),
    /// so real searches stay exhaustive; only a pathologically large
    /// table is truncated to a best-effort result. The lasting fix is a
    /// real search index (qdrant) rather than a scan — tracked in #35.
    pub async fn search_users(&self, query: &str) -> Result<Vec<User>, RepoError> {
        const MAX_MATCHES: usize = 10;
        // Cost guardrail: cap total rows examined across pages.
        const MAX_SCANNED_ITEMS: i32 = 5000;
        // Per-page evaluation cap so the budget is enforced at page
        // granularity and no single page reads an unbounded slice.
        const PAGE_LIMIT: i32 = 500;

        let query_lower = query.trim().to_lowercase();
        let mut out: Vec<User> = Vec::new();
        let mut scanned: i32 = 0;
        let mut last_key: Option<HashMap<String, AttributeValue>> = None;
        loop {
            let mut builder = self
                .db
                .inner()
                .scan()
                .table_name(self.db.table_name())
                .limit(PAGE_LIMIT)
                .filter_expression("SK = :sk AND (contains(email, :q) OR contains(#n, :q))")
                .expression_attribute_values(":sk", AttributeValue::S("PROFILE".to_string()))
                .expression_attribute_values(":q", AttributeValue::S(query_lower.clone()))
                .expression_attribute_names("#n", "name");
            if let Some(key) = last_key.take() {
                builder = builder.set_exclusive_start_key(Some(key));
            }
            let result = builder
                .send()
                .await
                .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;

            // `scanned_count` is rows *examined* (the RCU-bearing figure),
            // not rows matched — that's exactly what the budget bounds.
            scanned += result.scanned_count();

            if let Some(items) = result.items {
                for item in items {
                    out.push(user_from_item(&item)?);
                    if out.len() >= MAX_MATCHES {
                        return Ok(out);
                    }
                }
            }

            match result.last_evaluated_key {
                // Keep paging only while under the scan budget; otherwise
                // return a best-effort result rather than traverse the
                // whole table for a sparse-match query.
                Some(key) if scanned < MAX_SCANNED_ITEMS => last_key = Some(key),
                _ => return Ok(out),
            }
        }
    }

    /// Update a user's profile fields.
    pub async fn update(
        &self,
        user_id: &str,
        name: Option<&str>,
        avatar_url: Option<Option<&str>>,
        updated_at: i64,
    ) -> Result<(), RepoError> {
        let pk = format!("USER#{user_id}");
        let mut expr_parts = vec!["#updated_at = :updated_at".to_string()];
        let mut remove_parts = Vec::new();
        let mut values = HashMap::new();
        let mut names = HashMap::new();

        names.insert("#updated_at".to_string(), "updated_at".to_string());
        values.insert(
            ":updated_at".to_string(),
            AttributeValue::N(updated_at.to_string()),
        );

        if let Some(n) = name {
            expr_parts.push("#name = :name".to_string());
            names.insert("#name".to_string(), "name".to_string());
            values.insert(":name".to_string(), AttributeValue::S(n.to_string()));
        }

        match avatar_url {
            Some(Some(url)) => {
                expr_parts.push("avatar_url = :avatar_url".to_string());
                values.insert(
                    ":avatar_url".to_string(),
                    AttributeValue::S(url.to_string()),
                );
            }
            Some(None) => {
                remove_parts.push("avatar_url".to_string());
            }
            None => {} // Don't touch the field
        }

        let mut update_expr = format!("SET {}", expr_parts.join(", "));
        if !remove_parts.is_empty() {
            update_expr.push_str(&format!(" REMOVE {}", remove_parts.join(", ")));
        }

        self.db
            .update_item(&pk, User::sk(), &update_expr, values, Some(names))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Bind a SCIM `externalId` / SAML NameID onto an existing user
    /// row. Writes both `external_id` (the canonical field) and
    /// `external_id_gsi` (the sparse-GSI hash key, so the row
    /// becomes discoverable via `get_by_external_id`). Phase 4 M-E4
    /// piece D consumer.
    pub async fn set_external_id(
        &self,
        user_id: &str,
        external_id: &str,
    ) -> Result<(), RepoError> {
        let pk = format!("USER#{user_id}");
        let mut values = HashMap::new();
        values.insert(
            ":val".to_string(),
            AttributeValue::S(external_id.to_string()),
        );
        self.db
            .update_item(
                &pk,
                User::sk(),
                "SET external_id = :val, external_id_gsi = :val",
                values,
                None,
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Phase 5 M-P1 piece B: replace the user's `ui_prefs` blob
    /// wholesale. The HTTP layer's `PUT /users/me/prefs` handler
    /// is responsible for merge semantics (read current → apply
    /// partial → write merged) — this repo method just persists
    /// whatever payload it's given.
    pub async fn set_ui_prefs(
        &self,
        user_id: &str,
        prefs: &crate::models::user::UiPrefs,
        updated_at: i64,
    ) -> Result<(), RepoError> {
        let pk = format!("USER#{user_id}");
        let mut values = HashMap::new();
        values.insert(
            ":prefs".to_string(),
            AttributeValue::S(serde_json::to_string(prefs).unwrap()),
        );
        values.insert(
            ":updated_at".to_string(),
            AttributeValue::N(updated_at.to_string()),
        );
        self.db
            .update_item(
                &pk,
                User::sk(),
                "SET ui_prefs = :prefs, updated_at = :updated_at",
                values,
                None,
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Set or clear the user's self-set status (account-menu step 5).
    /// `Some` writes the JSON blob; `None` removes the attribute. The
    /// HTTP layer owns validation/caps and expiry semantics — this
    /// just persists. `status` is a DynamoDB reserved word, so it's
    /// referenced via the `#status` expression-attribute-name.
    pub async fn set_status(
        &self,
        user_id: &str,
        status: Option<&crate::models::user::UserStatus>,
        updated_at: i64,
    ) -> Result<(), RepoError> {
        let pk = format!("USER#{user_id}");
        let mut values = HashMap::new();
        values.insert(
            ":updated_at".to_string(),
            AttributeValue::N(updated_at.to_string()),
        );
        let mut names = HashMap::new();
        names.insert("#status".to_string(), "status".to_string());

        let update_expr = match status {
            Some(s) => {
                values.insert(
                    ":status".to_string(),
                    AttributeValue::S(serde_json::to_string(s).unwrap()),
                );
                "SET #status = :status, updated_at = :updated_at".to_string()
            }
            None => "SET updated_at = :updated_at REMOVE #status".to_string(),
        };

        self.db
            .update_item(&pk, User::sk(), &update_expr, values, Some(names))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Set the user's email-notification preference (account-menu
    /// step 6). The enum is stored as its lowercase serde tag, matching
    /// `user_to_item`'s write and what the notify worker reads.
    pub async fn set_email_notifications(
        &self,
        user_id: &str,
        pref: crate::models::NotifEmailPref,
        updated_at: i64,
    ) -> Result<(), RepoError> {
        let pk = format!("USER#{user_id}");
        let pref_str = serde_json::to_string(&pref)
            .unwrap()
            .trim_matches('"')
            .to_string();
        let mut values = HashMap::new();
        values.insert(":pref".to_string(), AttributeValue::S(pref_str));
        values.insert(
            ":updated_at".to_string(),
            AttributeValue::N(updated_at.to_string()),
        );
        self.db
            .update_item(
                &pk,
                User::sk(),
                "SET email_notifications = :pref, updated_at = :updated_at",
                values,
                None,
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    // ─── Admin operations ──────────────────────────────────────

    /// Write or clear the encrypted MFA secret. `None` clears the
    /// attribute entirely (used by disarm). Called during enroll
    /// (writes the freshly-generated secret) and during disarm
    /// (clears alongside `mfa_enrolled_at`).
    pub async fn set_mfa_secret(
        &self,
        user_id: &str,
        secret: Option<&EncryptedString>,
    ) -> Result<(), RepoError> {
        let pk = format!("USER#{user_id}");
        let mut values = HashMap::new();
        let mut names = HashMap::new();
        names.insert("#mfa".to_string(), "mfa_secret".to_string());

        let expr = if let Some(blob) = secret {
            let mut m = HashMap::new();
            m.insert("nonce".to_string(), AttributeValue::S(blob.nonce.clone()));
            m.insert("ct".to_string(), AttributeValue::S(blob.ct.clone()));
            values.insert(":val".to_string(), AttributeValue::M(m));
            "SET #mfa = :val"
        } else {
            "REMOVE #mfa"
        };

        self.db
            .update_item(&pk, User::sk(), expr, values, Some(names))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Set or clear the MFA enrollment timestamp. Setting marks the
    /// user as fully enrolled (verify endpoint); clearing marks them
    /// pre-enroll again (disarm endpoint).
    pub async fn set_mfa_enrolled_at(
        &self,
        user_id: &str,
        ts: Option<i64>,
    ) -> Result<(), RepoError> {
        let pk = format!("USER#{user_id}");
        let mut values = HashMap::new();
        let expr = if let Some(t) = ts {
            values.insert(":val".to_string(), AttributeValue::N(t.to_string()));
            "SET mfa_enrolled_at = :val"
        } else {
            "REMOVE mfa_enrolled_at"
        };
        self.db
            .update_item(&pk, User::sk(), expr, values, None)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Set the user's role. Promotion / demotion ultimately writes this
    /// single string attribute; the `is_admin()` derivation on the read
    /// path picks up the change on the next user-row fetch.
    pub async fn set_role(&self, user_id: &str, role: UserRole) -> Result<(), RepoError> {
        let pk = format!("USER#{user_id}");
        let mut values = HashMap::new();
        values.insert(
            ":val".to_string(),
            AttributeValue::S(
                serde_json::to_string(&role)
                    .unwrap()
                    .trim_matches('"')
                    .to_string(),
            ),
        );
        let mut names = HashMap::new();
        // `role` is a DynamoDB reserved word; alias via expression-attribute-names.
        names.insert("#role".to_string(), "role".to_string());
        self.db
            .update_item(&pk, User::sk(), "SET #role = :val", values, Some(names))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Thin wrapper over `set_role` — keeps the boolean-flavored API for
    /// existing promote/demote call sites. New code with finer-grained
    /// role needs should call `set_role` directly.
    pub async fn set_admin(&self, user_id: &str, is_admin: bool) -> Result<(), RepoError> {
        let role = if is_admin { UserRole::Admin } else { UserRole::User };
        self.set_role(user_id, role).await
    }

    /// Set the `is_disabled` flag on a user.
    pub async fn set_disabled(&self, user_id: &str, is_disabled: bool) -> Result<(), RepoError> {
        let pk = format!("USER#{user_id}");
        let mut values = HashMap::new();
        values.insert(
            ":val".to_string(),
            AttributeValue::Bool(is_disabled),
        );
        self.db
            .update_item(&pk, User::sk(), "SET is_disabled = :val", values, None)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// #148 — set the `ask_policy` on a user (three-state
    /// `Disabled` / `SystemOnly` / `SystemOrByok`). Gate for
    /// `/api/v1/ask` access; only admins should be able to flip
    /// this. Persisted on the User row so the policy survives
    /// restarts and scale-out.
    ///
    /// Also REMOVEs the legacy `ask_enabled` DDB attribute in
    /// the same update — every explicit policy write drops the
    /// pre-migration attribute so eventually the legacy
    /// derivation path in `User::ask_policy()` becomes dead
    /// code and can be deleted. Safe to remove an attribute
    /// that doesn't exist (DDB is a no-op).
    pub async fn set_ask_policy(
        &self,
        user_id: &str,
        policy: AskPolicy,
    ) -> Result<(), RepoError> {
        let pk = format!("USER#{user_id}");
        let mut values = HashMap::new();
        values.insert(
            ":val".to_string(),
            AttributeValue::S(
                serde_json::to_string(&policy)
                    .unwrap()
                    .trim_matches('"')
                    .to_string(),
            ),
        );
        self.db
            .update_item(
                &pk,
                User::sk(),
                "SET ask_policy = :val REMOVE ask_enabled",
                values,
                None,
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Set the OAuth provider (and optional provider-side subject id) on a
    /// user row. Called when upgrading legacy `Unknown`-provider rows to
    /// the provider they just successfully authenticated against.
    pub async fn set_provider(
        &self,
        user_id: &str,
        provider: super::super::models::user::AuthProvider,
        subject_id: Option<&str>,
    ) -> Result<(), RepoError> {
        let pk = format!("USER#{user_id}");
        let mut values = HashMap::new();
        values.insert(
            ":p".to_string(),
            AttributeValue::S(
                serde_json::to_string(&provider)
                    .unwrap()
                    .trim_matches('"')
                    .to_string(),
            ),
        );

        let expr = if let Some(sub) = subject_id {
            values.insert(
                ":s".to_string(),
                AttributeValue::S(sub.to_string()),
            );
            "SET provider = :p, provider_subject_id = :s"
        } else {
            "SET provider = :p REMOVE provider_subject_id"
        };

        self.db
            .update_item(&pk, User::sk(), expr, values, None)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Record the user's most recent authenticated request. Called from a
    /// debounced middleware — expect at most one write per user per 5
    /// minutes. The value feeds the "active in-app" suppression window
    /// used by the email notification service.
    pub async fn update_last_active_at(
        &self,
        user_id: &str,
        ts: i64,
    ) -> Result<(), RepoError> {
        let pk = format!("USER#{user_id}");
        let mut values = HashMap::new();
        values.insert(":ts".to_string(), AttributeValue::N(ts.to_string()));
        self.db
            .update_item(&pk, User::sk(), "SET last_active_at = :ts", values, None)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Set the user's default workspace id. Called by the first-login hook
    /// and by the backfill migration for pre-M1 users.
    pub async fn set_default_workspace(
        &self,
        user_id: &str,
        workspace_id: &str,
    ) -> Result<(), RepoError> {
        let pk = format!("USER#{user_id}");
        let mut values = HashMap::new();
        values.insert(
            ":ws".to_string(),
            AttributeValue::S(workspace_id.to_string()),
        );
        self.db
            .update_item(&pk, User::sk(), "SET default_workspace_id = :ws", values, None)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// List user `PROFILE` rows, paginated.
    ///
    /// This is a table `Scan` with a `SK = PROFILE` filter. DynamoDB
    /// applies a Scan's `Limit` to items examined *before* the filter, so
    /// a single page can come back with zero users while many more exist
    /// further along: the single table is dominated by non-`PROFILE` rows
    /// (folder, session, and CRDT op-log / snapshot rows). We therefore
    /// loop, following `LastEvaluatedKey`, accumulating matching users
    /// until we have a full page (`limit`) or the table is exhausted. The
    /// returned cursor is the PK of the last user emitted; resuming a Scan
    /// from a real item key continues correctly after it.
    pub async fn list_all(
        &self,
        limit: i32,
        cursor: Option<&str>,
    ) -> Result<(Vec<User>, Option<String>), RepoError> {
        let limit = limit.max(1) as usize;
        let mut users: Vec<User> = Vec::with_capacity(limit);

        // Scan resume key: seeded from the caller's cursor (the PK of the
        // last user from the previous page), then tracks DynamoDB's own
        // LastEvaluatedKey between pages within this call.
        let mut start_key: Option<HashMap<String, AttributeValue>> = cursor.map(|pk| {
            HashMap::from([
                ("PK".to_string(), AttributeValue::S(pk.to_string())),
                ("SK".to_string(), AttributeValue::S("PROFILE".to_string())),
            ])
        });

        loop {
            let mut builder = self
                .db
                .inner()
                .scan()
                .table_name(self.db.table_name())
                .filter_expression("SK = :sk")
                .expression_attribute_values(":sk", AttributeValue::S("PROFILE".to_string()));
            if let Some(ref key) = start_key {
                builder = builder.set_exclusive_start_key(Some(key.clone()));
            }

            let result = builder
                .send()
                .await
                .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;

            for item in result.items.unwrap_or_default().iter() {
                if let Ok(user) = user_from_item(item) {
                    users.push(user);
                    if users.len() == limit {
                        // Full page. Resume after this user on the next call.
                        let next = users.last().map(|u| u.pk());
                        return Ok((users, next));
                    }
                }
            }

            match result.last_evaluated_key {
                // More of the table left to scan; keep accumulating.
                Some(key) => start_key = Some(key),
                // Table exhausted; this is the last page.
                None => return Ok((users, None)),
            }
        }
    }
}

/// Sort key for the email→user pointer item (#36).
const EMAIL_POINTER_SK: &str = "POINTER";

/// Partition key for the `EMAIL#<lowercased> → user_id` pointer item (#36).
/// The caller lowercases + trims the email first, matching the old scan's
/// `email` comparison so lookups stay case-insensitive.
fn email_pointer_pk(email_lc: &str) -> String {
    format!("EMAIL#{email_lc}")
}

fn user_to_item(user: &User) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::new();
    item.insert("user_id".to_string(), AttributeValue::S(user.user_id.clone()));
    item.insert("name".to_string(), AttributeValue::S(user.name.clone()));
    item.insert("email".to_string(), AttributeValue::S(user.email.clone()));
    if let Some(ref url) = user.avatar_url {
        item.insert("avatar_url".to_string(), AttributeValue::S(url.clone()));
    }
    item.insert("home_folder_id".to_string(), AttributeValue::S(user.home_folder_id.clone()));
    item.insert("private_folder_id".to_string(), AttributeValue::S(user.private_folder_id.clone()));
    item.insert("trash_folder_id".to_string(), AttributeValue::S(user.trash_folder_id.clone()));
    if let Some(ref id) = user.archive_folder_id {
        item.insert("archive_folder_id".to_string(), AttributeValue::S(id.clone()));
    }
    if let Some(ref id) = user.pinned_folder_id {
        item.insert("pinned_folder_id".to_string(), AttributeValue::S(id.clone()));
    }
    if let Some(ref id) = user.default_workspace_id {
        item.insert("default_workspace_id".to_string(), AttributeValue::S(id.clone()));
    }
    // Sparse GSI: write both the semantic field and the GSI hash-key
    // attribute, only when `external_id` is Some. Rows without an
    // external_id (the vast majority pre-SCIM) stay out of the index
    // entirely — no storage cost, no fan-out churn.
    if let Some(ref ext) = user.external_id {
        item.insert("external_id".to_string(), AttributeValue::S(ext.clone()));
        item.insert("external_id_gsi".to_string(), AttributeValue::S(ext.clone()));
    }
    // Always write provider — serde's rename_all=lowercase emits strings
    // like "github" / "google" / "dev" / "unknown".
    item.insert(
        "provider".to_string(),
        AttributeValue::S(
            serde_json::to_string(&user.provider)
                .unwrap()
                .trim_matches('"')
                .to_string(),
        ),
    );
    if let Some(ref sub) = user.provider_subject_id {
        item.insert(
            "provider_subject_id".to_string(),
            AttributeValue::S(sub.clone()),
        );
    }
    // `role` is always written (no default-on-read shim). Stored as a
    // lowercase string ("user" / "admin") via serde_json — same pattern
    // as `provider` above.
    item.insert(
        "role".to_string(),
        AttributeValue::S(
            serde_json::to_string(&user.role)
                .unwrap()
                .trim_matches('"')
                .to_string(),
        ),
    );
    if user.is_disabled {
        item.insert("is_disabled".to_string(), AttributeValue::Bool(true));
    }
    // #148 — new writes stamp `ask_policy` explicitly whenever
    // the User carries a non-default policy. The legacy
    // `ask_enabled` attribute is never written by new code; the
    // `set_ask_policy` update REMOVEs it on any in-place flip so
    // it eventually falls out of every row.
    if let Some(policy) = user.ask_policy {
        item.insert(
            "ask_policy".to_string(),
            AttributeValue::S(
                serde_json::to_string(&policy)
                    .unwrap()
                    .trim_matches('"')
                    .to_string(),
            ),
        );
    }
    // MFA fields. The encrypted secret is written as a Map AV with
    // the same {nonce, ct} shape as the JSON serialization, so an
    // operator inspecting the row in the DynamoDB console sees a
    // self-describing structure (not an opaque blob).
    if let Some(ref blob) = user.mfa_secret {
        let mut map = HashMap::new();
        map.insert("nonce".to_string(), AttributeValue::S(blob.nonce.clone()));
        map.insert("ct".to_string(), AttributeValue::S(blob.ct.clone()));
        item.insert("mfa_secret".to_string(), AttributeValue::M(map));
    }
    if let Some(ts) = user.mfa_enrolled_at {
        item.insert(
            "mfa_enrolled_at".to_string(),
            AttributeValue::N(ts.to_string()),
        );
    }
    item.insert(
        "email_notifications".to_string(),
        AttributeValue::S(
            serde_json::to_string(&user.email_notifications)
                .unwrap()
                .trim_matches('"')
                .to_string(),
        ),
    );
    // Phase 5 M-P1 piece B: ui_prefs stored as a JSON-stringified
    // attribute. Mirrors `detail`-as-JSON-string in SecurityAudit.
    // Omitted when None to keep rows minimal for users who haven't
    // customized anything.
    if let Some(ref prefs) = user.ui_prefs {
        item.insert(
            "ui_prefs".to_string(),
            AttributeValue::S(serde_json::to_string(prefs).unwrap()),
        );
    }
    // Status: same JSON-string-blob pattern as ui_prefs (account-menu
    // step 5). Omitted when None.
    if let Some(ref status) = user.status {
        item.insert(
            "status".to_string(),
            AttributeValue::S(serde_json::to_string(status).unwrap()),
        );
    }
    if user.last_active_at != 0 {
        item.insert("last_active_at".to_string(), AttributeValue::N(user.last_active_at.to_string()));
    }
    item.insert("created_at".to_string(), AttributeValue::N(user.created_at.to_string()));
    item.insert("updated_at".to_string(), AttributeValue::N(user.updated_at.to_string()));
    item
}

fn user_from_item(item: &HashMap<String, AttributeValue>) -> Result<User, RepoError> {
    // Deserialize provider leniently: legacy rows written before the field
    // existed read back as `Unknown`, which find_or_create_user treats as
    // "accept and upgrade".
    let provider = item
        .get("provider")
        .and_then(|v| v.as_s().ok())
        .and_then(|s| serde_json::from_str(&format!("\"{s}\"")).ok())
        .unwrap_or_default();

    Ok(User {
        user_id: get_s(item, "user_id")?,
        name: get_s(item, "name")?,
        email: get_s(item, "email")?,
        avatar_url: item.get("avatar_url").and_then(|v| v.as_s().ok()).cloned(),
        provider,
        provider_subject_id: item
            .get("provider_subject_id")
            .and_then(|v| v.as_s().ok())
            .cloned(),
        home_folder_id: get_s(item, "home_folder_id")?,
        private_folder_id: get_s(item, "private_folder_id")?,
        trash_folder_id: get_s(item, "trash_folder_id")?,
        archive_folder_id: item.get("archive_folder_id").and_then(|v| v.as_s().ok()).cloned(),
        pinned_folder_id: item.get("pinned_folder_id").and_then(|v| v.as_s().ok()).cloned(),
        default_workspace_id: item.get("default_workspace_id").and_then(|v| v.as_s().ok()).cloned(),
        mfa_secret: item.get("mfa_secret").and_then(|v| v.as_m().ok()).and_then(|m| {
            let nonce = m.get("nonce").and_then(|v| v.as_s().ok())?;
            let ct = m.get("ct").and_then(|v| v.as_s().ok())?;
            Some(crate::models::user::EncryptedString {
                nonce: nonce.clone(),
                ct: ct.clone(),
            })
        }),
        mfa_enrolled_at: item
            .get("mfa_enrolled_at")
            .and_then(|v| v.as_n().ok())
            .and_then(|n| n.parse::<i64>().ok()),
        external_id: item.get("external_id").and_then(|v| v.as_s().ok()).cloned(),
        // Hard migration: the `role` attribute must exist on every row
        // post-backfill. A missing attribute is a deployment-order error,
        // not a legacy-row case to silently default — surface it.
        role: {
            // `get_s` already raises MissingField("role") when the attribute is
            // absent; the parse-failure branch below fires only when the
            // attribute is *present* but holds an unrecognized value (corrupt
            // write, manual edit, future variant not yet deployed). Surface
            // that distinctly so an operator doesn't waste time re-running a
            // backfill that wouldn't help.
            let s = get_s(item, "role")?;
            serde_json::from_str::<UserRole>(&format!("\"{s}\""))
                .map_err(|e| {
                    RepoError::MissingField(format!("role (unrecognized value {s:?}): {e}"))
                })?
        },
        is_disabled: item.get("is_disabled").and_then(|v| v.as_bool().ok()).copied().unwrap_or(false),
        // #148 — new `ask_policy` field. Absent on pre-migration
        // rows; the `User::ask_policy()` getter derives from the
        // legacy `ask_enabled` in that case.
        ask_policy: item
            .get("ask_policy")
            .and_then(|v| v.as_s().ok())
            .and_then(|s| serde_json::from_str(&format!("\"{s}\"")).ok()),
        legacy_ask_enabled: item
            .get("ask_enabled")
            .and_then(|v| v.as_bool().ok())
            .copied()
            .unwrap_or(false),
        email_notifications: item.get("email_notifications").and_then(|v| v.as_s().ok())
            .and_then(|s| serde_json::from_str(&format!("\"{s}\"")).ok())
            .unwrap_or_default(),
        // Phase 5 M-P1 piece B. JSON-stringified blob; absent on
        // every row written before this commit. Treat "absent" or
        // "unparseable" as `None` — a corrupted prefs row should
        // degrade gracefully to defaults, not block login.
        ui_prefs: item.get("ui_prefs").and_then(|v| v.as_s().ok())
            .and_then(|s| serde_json::from_str(s).ok()),
        // Status: JSON-string blob, same graceful-degrade posture as
        // ui_prefs — absent / unparseable reads as `None`.
        status: item.get("status").and_then(|v| v.as_s().ok())
            .and_then(|s| serde_json::from_str(s).ok()),
        last_active_at: item.get("last_active_at").and_then(|v| v.as_n().ok())
            .and_then(|n| n.parse::<i64>().ok()).unwrap_or(0),
        created_at: get_n(item, "created_at")?,
        updated_at: get_n(item, "updated_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::user::AuthProvider;
    use ogrenotes_common::id::new_id;
    use ogrenotes_common::time::now_usec;

    fn sample_user() -> User {
        let now = now_usec();
        User {
            user_id: new_id(),
            name: "Test User".to_string(),
            email: "test@example.com".to_string(),
            avatar_url: None,
            provider: AuthProvider::Unknown,
            provider_subject_id: None,
            home_folder_id: new_id(),
            private_folder_id: new_id(),
            trash_folder_id: new_id(),
            archive_folder_id: None,
            pinned_folder_id: None,
            default_workspace_id: None,
            mfa_secret: None,
            mfa_enrolled_at: None,
            external_id: None,
            role: UserRole::User,
            is_disabled: false,
            ask_policy: None,
            legacy_ask_enabled: false,
            email_notifications: Default::default(),
            ui_prefs: None,
            status: None,
            last_active_at: 0,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn user_to_item_writes_role_string() {
        let mut user = sample_user();
        user.role = UserRole::Admin;
        let item = user_to_item(&user);
        let role = item.get("role").expect("role attribute present");
        assert_eq!(role.as_s().unwrap(), "admin");
    }

    #[test]
    fn user_to_item_omits_external_id_when_none() {
        // Sparse-GSI invariant: rows without an external_id must not
        // emit the `external_id_gsi` attribute, otherwise they'd land
        // in the index (and bloat scans for the SCIM lookup case).
        let user = sample_user();
        let item = user_to_item(&user);
        assert!(!item.contains_key("external_id_gsi"));
        assert!(!item.contains_key("external_id"));
    }

    #[test]
    fn user_to_item_omits_status_when_none() {
        let user = sample_user();
        let item = user_to_item(&user);
        assert!(!item.contains_key("status"));
    }

    #[test]
    fn user_from_item_round_trips_email_notifications() {
        use crate::models::NotifEmailPref;
        // Pins the three serialization sites together: user_to_item's
        // write, user_from_item's read, and (by sharing the same
        // lowercase-tag convention) set_email_notifications.
        for pref in [
            NotifEmailPref::All,
            NotifEmailPref::MentionsOnly,
            NotifEmailPref::Disabled,
        ] {
            let mut user = sample_user();
            user.email_notifications = pref.clone();
            let item = user_to_item(&user);
            let back = user_from_item(&item).expect("round-trips");
            assert_eq!(back.email_notifications, pref);
        }
    }

    #[test]
    fn user_from_item_round_trips_status() {
        use crate::models::user::UserStatus;
        let mut user = sample_user();
        user.status = Some(UserStatus {
            text: "on vacation".to_string(),
            emoji: Some("🌴".to_string()),
            expires_at: Some(1_700_000_000_000_000),
        });
        let item = user_to_item(&user);
        let back = user_from_item(&item).expect("round-trips");
        assert_eq!(back.status, user.status);
    }

    #[test]
    fn user_to_item_writes_external_id_gsi_when_some() {
        let mut user = sample_user();
        user.external_id = Some("scim-user-42".to_string());
        let item = user_to_item(&user);
        assert_eq!(
            item.get("external_id").and_then(|v| v.as_s().ok()),
            Some(&"scim-user-42".to_string()),
        );
        assert_eq!(
            item.get("external_id_gsi").and_then(|v| v.as_s().ok()),
            Some(&"scim-user-42".to_string()),
        );
    }

    #[test]
    fn user_from_item_round_trips_external_id() {
        let mut user = sample_user();
        user.external_id = Some("scim-user-42".to_string());
        let item = user_to_item(&user);
        let back = user_from_item(&item).expect("from_item");
        assert_eq!(back.external_id.as_deref(), Some("scim-user-42"));
    }

    #[test]
    fn user_from_item_fails_when_role_missing() {
        let user = sample_user();
        let mut item = user_to_item(&user);
        item.remove("role");
        let err = user_from_item(&item).expect_err("must reject missing role");
        match err {
            RepoError::MissingField(f) => assert_eq!(f, "role"),
            other => panic!("expected MissingField(role), got {other:?}"),
        }
    }

    #[test]
    fn user_from_item_round_trips_fully_populated_user() {
        // One pass over the fully-populated shape: every optional
        // attribute written by user_to_item must decode back
        // identically. Catches an encode/decode drift on any single
        // field even without a focused test for it.
        use crate::models::user::{
            AskPolicy, EditorWidth, EncryptedString, ThemePref, UiPrefs, UserStatus,
        };
        let mut user = sample_user();
        user.avatar_url = Some("https://example.com/a.png".to_string());
        user.provider = AuthProvider::Github;
        user.provider_subject_id = Some("gh-12345".to_string());
        user.archive_folder_id = Some("f-arch".to_string());
        user.pinned_folder_id = Some("f-pin".to_string());
        user.default_workspace_id = Some("ws-1".to_string());
        user.mfa_secret = Some(EncryptedString {
            nonce: "bm9uY2U".to_string(),
            ct: "Y2lwaGVydGV4dA".to_string(),
        });
        user.mfa_enrolled_at = Some(1_700_000_000_000_000);
        user.external_id = Some("scim-42".to_string());
        user.role = UserRole::Admin;
        user.is_disabled = true;
        user.ask_policy = Some(AskPolicy::SystemOnly);
        user.ui_prefs = Some(UiPrefs {
            theme: Some(ThemePref::Dark),
            doc_theme: Some("editorial".to_string()),
            dyslexic_font: Some(true),
            reduce_motion: None,
            locale: Some("en-US".to_string()),
            editor_width: Some(EditorWidth::Wide),
        });
        user.status = Some(UserStatus {
            text: "afk".to_string(),
            emoji: None,
            expires_at: None,
        });
        user.last_active_at = 1_700_000_000_000_001;

        let back = user_from_item(&user_to_item(&user)).expect("from_item");
        assert_eq!(back, user);
    }

    #[test]
    fn user_from_item_defaults_absent_provider_to_unknown() {
        // Legacy rows written before provider tracking carry no
        // `provider` attribute; they must decode as Unknown (the
        // "accept and upgrade" state), not fail.
        let mut item = user_to_item(&sample_user());
        item.remove("provider");
        let back = user_from_item(&item).expect("from_item");
        assert_eq!(back.provider, AuthProvider::Unknown);
    }

    #[test]
    fn user_from_item_corrupt_ui_prefs_degrades_to_none() {
        // Documented graceful-degrade posture: a corrupted prefs blob
        // must read as None (defaults) rather than block login.
        let mut item = user_to_item(&sample_user());
        item.insert("ui_prefs".to_string(), AttributeValue::S("{broken".to_string()));
        let back = user_from_item(&item).expect("from_item");
        assert!(back.ui_prefs.is_none());
    }

    #[test]
    fn user_from_item_reads_legacy_ask_enabled_bool() {
        // #148 pre-migration row: no ask_policy attribute, legacy
        // `ask_enabled: true` Bool. Decode must populate
        // legacy_ask_enabled so the ask_policy() getter derives
        // SystemOrByok (the pre-migration behavior).
        use crate::models::user::AskPolicy;
        let mut item = user_to_item(&sample_user());
        assert!(!item.contains_key("ask_policy"));
        item.insert("ask_enabled".to_string(), AttributeValue::Bool(true));
        let back = user_from_item(&item).expect("from_item");
        assert!(back.ask_policy.is_none());
        assert!(back.legacy_ask_enabled);
        assert_eq!(back.ask_policy(), AskPolicy::SystemOrByok);
    }

    #[test]
    fn user_to_item_round_trips_every_ask_policy() {
        use crate::models::user::AskPolicy;
        for policy in [
            AskPolicy::Disabled,
            AskPolicy::SystemOnly,
            AskPolicy::SystemOrByok,
        ] {
            let mut user = sample_user();
            user.ask_policy = Some(policy);
            let back = user_from_item(&user_to_item(&user))
                .unwrap_or_else(|e| panic!("roundtrip failed for {policy:?}: {e}"));
            assert_eq!(back.ask_policy, Some(policy));
        }
    }

    #[test]
    fn user_to_item_mfa_secret_is_a_self_describing_map() {
        // The encrypted secret is stored as an M AttributeValue with
        // {nonce, ct} keys (not an opaque string) — pinned because
        // both this writer and set_mfa_secret must produce the same
        // shape user_from_item reads.
        use crate::models::user::EncryptedString;
        let mut user = sample_user();
        user.mfa_secret = Some(EncryptedString {
            nonce: "n1".to_string(),
            ct: "c1".to_string(),
        });
        let item = user_to_item(&user);
        let m = item
            .get("mfa_secret")
            .and_then(|v| v.as_m().ok())
            .expect("mfa_secret must be an M attribute");
        assert_eq!(m.get("nonce").and_then(|v| v.as_s().ok()).map(String::as_str), Some("n1"));
        assert_eq!(m.get("ct").and_then(|v| v.as_s().ok()).map(String::as_str), Some("c1"));
        let back = user_from_item(&item).expect("from_item");
        assert_eq!(back.mfa_secret, user.mfa_secret);
    }

    #[test]
    fn user_to_item_is_disabled_false_is_sparse() {
        // `is_disabled: false` writes no attribute (the shape every
        // healthy row has); absence decodes to false/enabled.
        let item = user_to_item(&sample_user());
        assert!(!item.contains_key("is_disabled"));
        let back = user_from_item(&item).expect("from_item");
        assert!(!back.is_disabled);
    }

    #[test]
    fn user_from_item_reports_invalid_role_distinctly_from_missing() {
        // A corrupt or future-tier value should NOT surface as `MissingField("role")`
        // — that would mislead an operator into re-running a backfill that
        // can't help. The message must mention the bad value so the real
        // problem is visible.
        let user = sample_user();
        let mut item = user_to_item(&user);
        item.insert("role".to_string(), AttributeValue::S("superadmin".to_string()));
        let err = user_from_item(&item).expect_err("must reject unknown role value");
        match err {
            RepoError::MissingField(f) => {
                assert!(f.contains("superadmin"), "expected value in error, got: {f}");
                assert_ne!(f, "role", "message must distinguish from absent-field case");
            }
            other => panic!("expected MissingField(...), got {other:?}"),
        }
    }
}
