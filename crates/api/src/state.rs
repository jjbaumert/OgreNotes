use ogrenotes_common::config::AppConfig;
use ogrenotes_storage::dynamo::DynamoClient;
use ogrenotes_storage::repo::doc_repo::DocRepo;
use ogrenotes_storage::repo::folder_repo::FolderRepo;
use ogrenotes_storage::repo::session_repo::SessionRepo;
use ogrenotes_storage::repo::user_repo::UserRepo;
use ogrenotes_storage::s3::S3Client;
use std::sync::Arc;

/// Shared application state passed to all Axum handlers.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub user_repo: Arc<UserRepo>,
    pub doc_repo: Arc<DocRepo>,
    pub folder_repo: Arc<FolderRepo>,
    pub session_repo: Arc<SessionRepo>,
}

impl AppState {
    pub fn new(
        config: AppConfig,
        dynamo: DynamoClient,
        s3: S3Client,
    ) -> Self {
        let user_repo = Arc::new(UserRepo::new(dynamo.clone()));
        let doc_repo = Arc::new(DocRepo::new(dynamo.clone(), s3));
        let folder_repo = Arc::new(FolderRepo::new(dynamo.clone()));
        let session_repo = Arc::new(SessionRepo::new(dynamo));

        Self {
            config: Arc::new(config),
            user_repo,
            doc_repo,
            folder_repo,
            session_repo,
        }
    }
}
