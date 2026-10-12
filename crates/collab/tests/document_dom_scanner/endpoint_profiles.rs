// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use super::*;

struct ProfileFiles(std::path::PathBuf);
impl ProfileFiles {
    fn new(contents: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "dom-endpoint-profile-{}-{}",
            std::process::id(),
            ogrenotes_common::time::now_usec()
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("config"), contents).unwrap();
        std::fs::write(path.join("credentials"), "").unwrap();
        Self(path)
    }

    fn configure(&self, command: &mut Command) {
        command
            .env("AWS_CONFIG_FILE", self.0.join("config"))
            .env("AWS_SHARED_CREDENTIALS_FILE", self.0.join("credentials"))
            .env("AWS_PROFILE", "scanner")
            .env_remove("AWS_ENDPOINT_URL")
            .env_remove("AWS_ENDPOINT_URL_DYNAMODB")
            .env_remove("AWS_ENDPOINT_URL_S3")
            .env_remove("AWS_IGNORE_CONFIGURED_ENDPOINT_URLS");
    }
}
impl Drop for ProfileFiles {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn profile_service_endpoints_match_header_and_reach_both_local_services() {
    if !available() {
        return;
    }
    let fixture = Fixture::new().await;
    fixture
        .doc("profile-doc", OgreDoc::new().to_state_bytes())
        .await;
    // The unusable global endpoint also proves service overrides take priority.
    let profiles = ProfileFiles::new(
        "[profile scanner]\nregion = us-east-1\nendpoint_url = http://127.0.0.1:1\nservices = scanner-local\n\n[services scanner-local]\ndynamodb =\n  endpoint_url = http://127.0.0.1:8000\ns3 =\n  endpoint_url = http://127.0.0.1:9000\n",
    );
    let mut command = fixture.command();
    profiles.configure(&mut command);
    let (status, rows) = run(command.args(["--doc-id", "profile-doc"]));
    let header = rows.iter().find(|r| r["kind"] == "header").unwrap();
    assert_eq!(header["dynamodb_endpoint"], "http://127.0.0.1:8000/");
    assert_eq!(header["s3_endpoint"], "http://127.0.0.1:9000/");
    assert_eq!(status, 0, "{rows:?}");
    assert_eq!(report(&rows, "profile-doc")["status"], "valid");
}

#[test]
fn profile_endpoint_header_redacts_credentials_query_and_fragment() {
    // Bind but never accept: even an SDK that accepts URL userinfo can only
    // contact this loopback listener, and its operation deadline bounds the run.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let authority = listener.local_addr().unwrap();
    let profiles = ProfileFiles::new(&format!(
        "[profile scanner]\nendpoint_url = http://PrivateEndpointUser:PrivateEndpointPassword@{authority}/prefix?PrivateEndpointQuery=secret#PrivateEndpointFragment\n"
    ));
    let mut command = Command::new(env!("CARGO_BIN_EXE_scan_document_dom"));
    profiles.configure(&mut command);
    command
        .env("AWS_ACCESS_KEY_ID", "local-test")
        .env("AWS_SECRET_ACCESS_KEY", "local-test")
        .env("AWS_EC2_METADATA_DISABLED", "true")
        .env_remove("AWS_SESSION_TOKEN")
        .env_remove("AWS_SECURITY_TOKEN");
    let (status, rows) = run(command.args([
        "--table",
        "local-table",
        "--bucket",
        "local-bucket",
        "--region",
        "us-east-1",
        "--doc-id",
        "local-doc",
        "--timeout-seconds",
        "1",
    ]));
    assert_eq!(status, 2, "unavailable loopback service must not pass");
    let header = rows.iter().find(|r| r["kind"] == "header").unwrap();
    let expected = format!("http://{authority}/prefix");
    assert_eq!(header["dynamodb_endpoint"], expected);
    assert_eq!(header["s3_endpoint"], expected);
    assert!(
        !serde_json::to_string(&rows)
            .unwrap()
            .contains("PrivateEndpoint")
    );
}
