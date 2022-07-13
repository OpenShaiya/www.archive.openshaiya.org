use aws_smithy_http::byte_stream::ByteStream;
use clientbuilder::{build_client, Distribution, AWS_S3_BUCKET};
use lambda_http::http::StatusCode;
use lambda_http::{service_fn, Body, Error, IntoResponse, Request, RequestExt, Response};
use serde::{Deserialize, Serialize};
use sqlite::Connection;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

/// The base s3 url where files are stored.
const ARCHIVE_URL: &str = "https://s3.amazonaws.com/archive.openshaiya.org";

/// The object key for the sqlite database.
const DATABASE_KEY: &str = "api/archive.sqlite";

#[derive(Deserialize)]
struct SRequest {
    dist: Distribution,
    patch: u16,
}

#[derive(Serialize)]
struct SResponse {
    url: String,
    elapsed: Duration,
}

impl IntoResponse for SResponse {
    fn into_response(self) -> Response<Body> {
        let body = serde_json::to_string(&self).unwrap();
        Response::builder()
            .status(StatusCode::OK)
            .body(Body::Text(body))
            .unwrap()
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt::init();
    let func = service_fn(handler);
    lambda_http::run(func).await.unwrap();
    Ok(())
}

async fn handler(http_req: Request) -> Result<impl IntoResponse, Error> {
    let req: SRequest = http_req.payload().unwrap_or(None).unwrap();

    // Initialise an s3 client.
    let aws_config = aws_config::load_from_env().await;
    let s3_client = aws_sdk_s3::Client::new(&aws_config);

    // Even within the same region, downloading thousands of files from S3 is painfully slow. To
    // circumvent this, we have mounted a local copy of the archive in an EFS filesystem, and
    // will be used that to read the data.
    let archive_path = std::env::var("ARCHIVE_PATH")?;
    let efs_path = Path::new(&archive_path);
    let tmp = std::env::temp_dir();

    // Initialise the database.
    let conn = init_db(efs_path).await?;
    let time = Instant::now();

    // Normalise the patch number and get the object key.
    let patch = clientbuilder::normalize_patch(&conn, req.dist, req.patch)?;
    let key = format!(
        "api/build/{}.tar.gz",
        clientbuilder::object_name(req.dist, patch)
    );
    let url = format!("{}/{}", ARCHIVE_URL, &key);

    // If a file with the specified key already exists, we can just return with that file.
    if (s3_client
        .head_object()
        .bucket(AWS_S3_BUCKET)
        .key(&key)
        .send()
        .await)
        .is_ok()
    {
        return Ok(SResponse {
            url,
            elapsed: time.elapsed(),
        });
    }

    // Build the client
    let client = build_client(&conn, &tmp, efs_path, req.dist, patch, None)
        .await
        .unwrap();
    let metadata = fs::metadata(&client).unwrap();
    let stream = ByteStream::from_path(&client).await.unwrap();
    tracing::info!(?client, len = metadata.len(), "built client; uploading");

    // Upload the client
    s3_client
        .put_object()
        .bucket(AWS_S3_BUCKET)
        .key(&key)
        .body(stream)
        .send()
        .await
        .unwrap();

    Ok(SResponse {
        url,
        elapsed: time.elapsed(),
    })
}

/// Initialise the sqlite database, from a file at a provided path.
///
/// # Arguments
/// * `path`    - The database path.
async fn init_db(path: &Path) -> anyhow::Result<Connection> {
    let db_path = path.join(DATABASE_KEY);
    Ok(sqlite::open(&db_path)?)
}
