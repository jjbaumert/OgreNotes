use ogrenotes_collab::redis_pubsub::RedisPubSub;
use ogrenotes_collab::room::RoomRegistry;
use ogrenotes_common::config::AppConfig;
use ogrenotes_storage::dynamo::DynamoClient;
use ogrenotes_storage::repo::doc_repo::DocRepo;
use ogrenotes_storage::repo::folder_repo::FolderRepo;
use ogrenotes_storage::repo::notification_repo::NotificationRepo;
use ogrenotes_storage::repo::session_repo::SessionRepo;
use ogrenotes_storage::repo::snapshot_repo::SnapshotRepo;
use ogrenotes_storage::repo::thread_repo::ThreadRepo;
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
    pub thread_repo: Arc<ThreadRepo>,
    pub notification_repo: Arc<NotificationRepo>,
    pub snapshot_repo: Arc<SnapshotRepo>,
    pub room_registry: Arc<RoomRegistry>,
    pub redis_pubsub: Arc<RedisPubSub>,
}

impl AppState {
    pub fn new(
        config: AppConfig,
        dynamo: DynamoClient,
        s3: S3Client,
        redis_pubsub: RedisPubSub,
    ) -> Self {
        let user_repo = Arc::new(UserRepo::new(dynamo.clone()));
        let doc_repo = Arc::new(DocRepo::new(dynamo.clone(), s3));
        let folder_repo = Arc::new(FolderRepo::new(dynamo.clone()));
        let thread_repo = Arc::new(ThreadRepo::new(dynamo.clone()));
        let notification_repo = Arc::new(NotificationRepo::new(dynamo.clone()));
        let snapshot_repo = Arc::new(SnapshotRepo::new(dynamo.clone()));
        let session_repo = Arc::new(SessionRepo::new(dynamo));

        Self {
            config: Arc::new(config),
            user_repo,
            doc_repo,
            folder_repo,
            session_repo,
            thread_repo,
            notification_repo,
            snapshot_repo,
            room_registry: Arc::new(RoomRegistry::new()),
            redis_pubsub: Arc::new(redis_pubsub),
        }
    }
}
