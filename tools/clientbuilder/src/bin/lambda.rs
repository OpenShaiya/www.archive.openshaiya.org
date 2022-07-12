use aws_smithy_http::byte_stream::ByteStream;
use clientbuilder::{build_client, Distribution, AWS_S3_BUCKET};
use lambda_runtime::{service_fn, LambdaEvent};
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
struct Request {
    dist: Distribution,
    patch: u16,
}

#[derive(Serialize)]
struct Response {
    url: String,
    elapsed: Duration,
}

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    tracing_subscriber::fmt::init();
    let func = service_fn(handler);
    lambda_runtime::run(func).await?;
    Ok(())
}

async fn handler(event: LambdaEvent<Request>) -> anyhow::Result<Response> {
    let (req, _ctx) = event.into_parts();

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
    let conn = init_db(&efs_path).await?;
    let time = Instant::now();

    // Normalise the patch number and get the object key.
    let patch = clientbuilder::normalize_patch(&conn, req.dist, req.patch)?;
    let key = format!(
        "api/build/{}.zip",
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
        return Ok(Response {
            url,
            elapsed: time.elapsed(),
        });
    }

    // Build the client
    let client = build_client(&conn, &tmp, efs_path, req.dist, patch, None).await?;
    tracing::info!(?client, "built client, reading data...");
    let stream = ByteStream::from_path(&client).await?;
    tracing::info!("loaded data into memory; uploading...");

    // Upload the client
    s3_client
        .put_object()
        .bucket(AWS_S3_BUCKET)
        .key(&key)
        .body(stream)
        .send()
        .await?;

    // Delete the archived file.
    fs::remove_file(&client)?;
    Ok(Response {
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
