//! Desktop ⇄ web sync, the Rust half. The sync loop (what to pull, push and
//! merge) runs in the frontend (`src/sync`); these commands do what it
//! can't:
//!
//! - sign in to the web app with a device code and keep the session token
//!   in the Keychain, never handing it to the webview;
//! - call the web app's API with that token ([`sync_request`]);
//! - read a guide and the copies of its images that leave the device, with
//!   blurs burned in ([`sync_prepare_guide`]), and upload them;
//! - write a pulled guide into the library: missing images first, then
//!   `guide.json` atomically, never into a guide open in the editor
//!   ([`sync_apply_guide`]).
//!
//! Nothing syncs unless the user turns it on; the frontend decides which
//! guides (`sync-state.json`, opaque here).

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use guiderecorder_core::ai::keychain::{self, ApiKey, SYNC_ACCOUNT};
use guiderecorder_core::edit::EditError;
use guiderecorder_core::library::{write_atomic, Durability, LibraryError};
use guiderecorder_core::model::Guide;
use guiderecorder_core::sync::{image_paths, sha256_hex, upload_image};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::State;

use crate::blocking::blocking;
use crate::editor::{parse_id, AppError, ErrorKind, Result, SessionCell};

/// The desktop app's client id for device authorization (the server's
/// `DESKTOP_CLIENT_ID`).
pub const CLIENT_ID: &str = "guiderecorder-desktop";
const ACCOUNT_FILE: &str = "sync-account.json";
const STATE_FILE: &str = "sync-state.json";

/// The folder `settings.json` lives in.
fn config_dir() -> Result<PathBuf> {
    crate::settings::default_path()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .ok_or_else(|| AppError::new(ErrorKind::Io, "no home folder"))
}

fn io_error(context: &str) -> impl Fn(std::io::Error) -> AppError + '_ {
    move |e| AppError::new(ErrorKind::Io, format!("{context}: {e}"))
}

fn http_error(e: reqwest::Error) -> AppError {
    AppError::new(ErrorKind::Io, format!("can't reach the server: {e}"))
}

/// Where the session token was issued. The token is in the Keychain.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncAccount {
    /// The web app's origin, e.g. `https://gr.dufflebear.dev`.
    pub server: Option<String>,
    pub signed_in: bool,
}

fn read_account(dir: &Path) -> SyncAccount {
    fs::read(dir.join(ACCOUNT_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_account(dir: &Path, account: &SyncAccount) -> Result<()> {
    fs::create_dir_all(dir).map_err(io_error("cannot create the settings folder"))?;
    let json = serde_json::to_vec_pretty(account).expect("serializable");
    write_atomic(&dir.join(ACCOUNT_FILE), &json, Durability::Full).map_err(Into::into)
}

/// A server origin: `https://host[:port]`, or `http://` for localhost only.
pub fn normalize_server(server: &str) -> Result<String> {
    let invalid = || {
        AppError::new(
            ErrorKind::Invalid,
            "enter the web app's address, e.g. https://gr.dufflebear.dev",
        )
    };
    let url = reqwest::Url::parse(server.trim()).map_err(|_| invalid())?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    let secure = url.scheme() == "https" || (url.scheme() == "http" && local);
    if !secure || url.host_str().is_none() || !url.username().is_empty() {
        return Err(invalid());
    }
    Ok(url.origin().ascii_serialization())
}

/// The signed-in server and its token.
fn credentials() -> Result<(String, ApiKey)> {
    let account = read_account(&config_dir()?);
    let not_signed_in = || AppError::new(ErrorKind::Invalid, "sign in to the web app first");
    let server = account
        .server
        .filter(|_| account.signed_in)
        .ok_or_else(not_signed_in)?;
    let token = keychain::get_key(SYNC_ACCOUNT)
        .map_err(|e| AppError::new(ErrorKind::Io, e.to_string()))?
        .ok_or_else(not_signed_in)?;
    Ok((server, token))
}

/// HTTP and the upload copies of images waiting to be sent.
pub struct SyncState {
    http: reqwest::Client,
    /// SHA-256 → the bytes to upload, in a temp folder.
    uploads: Mutex<HashMap<String, PathBuf>>,
    upload_dir: PathBuf,
}

impl Default for SyncState {
    fn default() -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent(concat!("GuideRecorder/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("TLS backend available"),
            uploads: Mutex::new(HashMap::new()),
            upload_dir: std::env::temp_dir()
                .join(format!("guiderecorder-sync-{}", std::process::id())),
        }
    }
}

/// The server the app is signed in to, if any.
#[tauri::command]
pub async fn sync_account() -> Result<SyncAccount> {
    blocking(|| {
        let mut account = read_account(&config_dir()?);
        // The Keychain entry is the truth: it can be removed outside the app.
        account.signed_in &= keychain::get_key(SYNC_ACCOUNT).is_ok_and(|k| k.is_some());
        Ok(account)
    })
    .await
}

/// A device code to show while the user approves it in the browser.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    pub interval: u64,
}

/// Starts signing in to `server`: asks for a device code and opens the
/// approval page in the browser. Then poll [`sync_sign_in_poll`].
#[tauri::command]
pub async fn sync_sign_in_start(
    state: State<'_, Arc<SyncState>>,
    server: String,
) -> Result<DeviceCode> {
    let server = normalize_server(&server)?;
    let response = state
        .http
        .post(format!("{server}/api/auth/device/code"))
        .json(&serde_json::json!({ "client_id": CLIENT_ID }))
        .send()
        .await
        .map_err(http_error)?;
    if !response.status().is_success() {
        return Err(AppError::new(
            ErrorKind::Io,
            format!(
                "{server} doesn't look like a GuideRecorder web app ({})",
                response.status()
            ),
        ));
    }
    #[derive(Deserialize)]
    struct Wire {
        device_code: String,
        user_code: String,
        verification_uri: String,
        verification_uri_complete: String,
        expires_in: u64,
        interval: u64,
    }
    let wire: Wire = response.json().await.map_err(http_error)?;
    // Absolute, on the same server: never open a page the response picked.
    let absolute = |uri: &str| {
        let url = reqwest::Url::parse(&server)
            .and_then(|base| base.join(uri))
            .ok();
        url.filter(|u| u.origin().ascii_serialization() == server)
            .map(String::from)
    };
    let complete = absolute(&wire.verification_uri_complete).ok_or_else(|| {
        AppError::new(ErrorKind::Io, "the server sent an unexpected sign-in page")
    })?;
    let code = DeviceCode {
        device_code: wire.device_code,
        user_code: wire.user_code,
        verification_uri: absolute(&wire.verification_uri).unwrap_or_else(|| complete.clone()),
        verification_uri_complete: complete.clone(),
        expires_in: wire.expires_in,
        interval: wire.interval,
    };
    blocking(move || {
        let dir = config_dir()?;
        // A new server: forget the old sign-in.
        let account = read_account(&dir);
        if account.server.as_deref() != Some(&server) {
            keychain::delete_key(SYNC_ACCOUNT)
                .map_err(|e| AppError::new(ErrorKind::Io, e.to_string()))?;
        }
        write_account(
            &dir,
            &SyncAccount {
                server: Some(server),
                signed_in: false,
            },
        )?;
        tauri_plugin_opener::open_url(&complete, None::<&str>)
            .map_err(|e| AppError::new(ErrorKind::Io, format!("cannot open the browser: {e}")))
    })
    .await?;
    Ok(code)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SignInStatus {
    Pending,
    /// Wait longer between polls.
    SlowDown,
    Approved,
    Denied,
    Expired,
}

/// Checks whether the user approved the device code; on approval, stores the
/// session token in the Keychain.
#[tauri::command]
pub async fn sync_sign_in_poll(
    state: State<'_, Arc<SyncState>>,
    device_code: String,
) -> Result<SignInStatus> {
    let account = blocking(|| Ok(read_account(&config_dir()?))).await?;
    let server = account
        .server
        .ok_or_else(|| AppError::new(ErrorKind::Invalid, "start signing in first"))?;
    let response = state
        .http
        .post(format!("{server}/api/auth/device/token"))
        .json(&serde_json::json!({
            "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
            "device_code": device_code,
            "client_id": CLIENT_ID,
        }))
        .send()
        .await
        .map_err(http_error)?;
    let body: Value = response.json().await.map_err(http_error)?;
    if let Some(token) = body.get("access_token").and_then(Value::as_str) {
        let token = ApiKey::new(token).map_err(|e| AppError::new(ErrorKind::Io, e.to_string()))?;
        blocking(move || {
            keychain::set_key(SYNC_ACCOUNT, &token)
                .map_err(|e| AppError::new(ErrorKind::Io, e.to_string()))?;
            write_account(
                &config_dir()?,
                &SyncAccount {
                    server: Some(server),
                    signed_in: true,
                },
            )
        })
        .await?;
        return Ok(SignInStatus::Approved);
    }
    Ok(match body.get("error").and_then(Value::as_str) {
        Some("authorization_pending") => SignInStatus::Pending,
        Some("slow_down") => SignInStatus::SlowDown,
        Some("access_denied") => SignInStatus::Denied,
        Some("expired_token") => SignInStatus::Expired,
        _ => {
            let message = body
                .get("error_description")
                .and_then(Value::as_str)
                .unwrap_or("sign-in failed");
            return Err(AppError::new(ErrorKind::Io, message.to_owned()));
        }
    })
}

/// Opens a page of the signed-in web app (`path`, e.g. `/profile`) in the
/// browser.
#[tauri::command]
pub async fn sync_open_web(path: String) -> Result<()> {
    if !path.starts_with('/') || path.starts_with("//") || path.contains("..") {
        return Err(AppError::new(ErrorKind::Invalid, "not a web app page"));
    }
    let (server, _) = blocking(credentials).await?;
    tauri_plugin_opener::open_url(format!("{server}{path}"), None::<&str>)
        .map_err(|e| AppError::new(ErrorKind::Io, format!("can't open the browser: {e}")))
}

/// Signs out: ends the session on the server (best effort) and removes the
/// token from the Keychain.
#[tauri::command]
pub async fn sync_sign_out(state: State<'_, Arc<SyncState>>) -> Result<()> {
    if let Ok((server, token)) = blocking(credentials).await {
        let _ = state
            .http
            .post(format!("{server}/api/auth/sign-out"))
            .bearer_auth(token.expose())
            .header("origin", &server)
            .send()
            .await;
    }
    blocking(|| {
        keychain::delete_key(SYNC_ACCOUNT)
            .map_err(|e| AppError::new(ErrorKind::Io, e.to_string()))?;
        let dir = config_dir()?;
        let account = read_account(&dir);
        write_account(
            &dir,
            &SyncAccount {
                server: account.server,
                signed_in: false,
            },
        )
    })
    .await
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiResponse {
    pub status: u16,
    /// The JSON body; null if there is none.
    pub body: Value,
}

/// Calls the web app's API (`path` under `/api/`) as the signed-in user.
#[tauri::command]
pub async fn sync_request(
    state: State<'_, Arc<SyncState>>,
    method: String,
    path: String,
    body: Option<Value>,
) -> Result<ApiResponse> {
    if !path.starts_with("/api/") || path.contains("..") {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "only API paths can be requested",
        ));
    }
    let method = match method.as_str() {
        "GET" => reqwest::Method::GET,
        "POST" => reqwest::Method::POST,
        "DELETE" => reqwest::Method::DELETE,
        _ => {
            return Err(AppError::new(
                ErrorKind::Invalid,
                format!("unsupported method {method}"),
            ))
        }
    };
    let (server, token) = blocking(credentials).await?;
    let mut request = state
        .http
        .request(method, format!("{server}{path}"))
        .bearer_auth(token.expose())
        // Better Auth checks the origin of state-changing calls.
        .header("origin", &server);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.map_err(http_error)?;
    let status = response.status().as_u16();
    let text = response.text().await.map_err(http_error)?;
    let body = serde_json::from_str(&text).unwrap_or(Value::Null);
    Ok(ApiResponse { status, body })
}

/// The frontend's sync state: which guides sync, their last synced
/// revisions, the change feed cursor. Opaque here.
#[tauri::command]
pub async fn sync_load_state() -> Result<Value> {
    blocking(|| {
        let path = config_dir()?.join(STATE_FILE);
        Ok(fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or(Value::Null))
    })
    .await
}

#[tauri::command]
pub async fn sync_save_state(state: Value) -> Result<()> {
    blocking(move || {
        let dir = config_dir()?;
        fs::create_dir_all(&dir).map_err(io_error("cannot create the settings folder"))?;
        let json = serde_json::to_vec(&state).expect("serializable");
        write_atomic(&dir.join(STATE_FILE), &json, Durability::Full).map_err(Into::into)
    })
    .await
}

/// A guide as it is on disk.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalGuide {
    pub guide: Guide,
    pub path: PathBuf,
    /// From a newer app, or not yet upgraded: don't sync it.
    pub read_only: bool,
}

/// The saved copy of guide `id`, or null if the library doesn't have it.
#[tauri::command]
pub async fn sync_read_guide(
    cell: State<'_, Arc<SessionCell>>,
    id: String,
) -> Result<Option<LocalGuide>> {
    let cell = Arc::clone(&cell);
    blocking(move || {
        let session = cell.get()?;
        let id = parse_id(&id)?;
        let Some(handle) = session.library().find(id)? else {
            return Ok(None);
        };
        let loaded = session.library().read(&handle.path)?;
        Ok(Some(LocalGuide {
            guide: loaded.guide,
            path: loaded.handle.path,
            read_only: loaded.read_only,
        }))
    })
    .await
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedImage {
    pub path: String,
    pub sha256: String,
    pub size: u64,
    pub content_type: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedGuide {
    pub guide: Guide,
    pub images: Vec<PreparedImage>,
}

/// Reads guide `id` and makes the upload copy of every image it names, with
/// blurs burned in, ready for [`sync_upload`].
#[tauri::command]
pub async fn sync_prepare_guide(
    cell: State<'_, Arc<SessionCell>>,
    state: State<'_, Arc<SyncState>>,
    id: String,
) -> Result<PreparedGuide> {
    let (cell, state) = (Arc::clone(&cell), Arc::clone(&state));
    blocking(move || {
        let session = cell.get()?;
        let id = parse_id(&id)?;
        let handle = session
            .library()
            .find(id)?
            .ok_or(LibraryError::NotFound(id))?;
        let loaded = session.library().read(&handle.path)?;
        if loaded.read_only {
            return Err(AppError::new(
                ErrorKind::ReadOnly,
                format!("guide {id} can't sync until it's upgraded"),
            ));
        }
        fs::create_dir_all(&state.upload_dir).map_err(io_error("cannot create a temp folder"))?;
        let mut images = Vec::new();
        for (step, path) in image_paths(&loaded.guide) {
            let upload = upload_image(&loaded.handle, step, path)?;
            let file = state.upload_dir.join(&upload.sha256);
            if !file.exists() {
                write_atomic(&file, &upload.bytes, Durability::RenameOnly)?;
            }
            state
                .uploads
                .lock()
                .unwrap()
                .insert(upload.sha256.clone(), file);
            images.push(PreparedImage {
                path: upload.path,
                size: upload.bytes.len() as u64,
                sha256: upload.sha256,
                content_type: content_type(path),
            });
        }
        Ok(PreparedGuide {
            guide: loaded.guide,
            images,
        })
    })
    .await
}

fn content_type(path: &str) -> &'static str {
    match Path::new(path).extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        _ => "image/webp",
    }
}

/// A presigned upload from the server's `assets/uploads`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Upload {
    pub sha256: String,
    pub url: String,
    pub headers: BTreeMap<String, String>,
}

/// PUTs prepared images to their presigned URLs.
#[tauri::command]
pub async fn sync_upload(state: State<'_, Arc<SyncState>>, uploads: Vec<Upload>) -> Result<()> {
    for upload in uploads {
        let file = state
            .uploads
            .lock()
            .unwrap()
            .get(&upload.sha256)
            .cloned()
            .ok_or_else(|| {
                AppError::new(
                    ErrorKind::Invalid,
                    format!("image {} wasn't prepared", upload.sha256),
                )
            })?;
        let bytes =
            blocking(move || fs::read(&file).map_err(io_error("cannot read a prepared image")))
                .await?;
        let mut request = state.http.put(&upload.url).body(bytes);
        for (name, value) in &upload.headers {
            request = request.header(name, value);
        }
        let response = request.send().await.map_err(http_error)?;
        if !response.status().is_success() {
            return Err(AppError::new(
                ErrorKind::Io,
                format!("image upload failed ({})", response.status()),
            ));
        }
    }
    Ok(())
}

/// Writes a guide pulled from the web app into the library: downloads the
/// images it names that the bundle lacks, then saves `guide.json`, creating
/// the bundle if the library doesn't have the guide. Images already on disk
/// are kept: they are the originals the uploads were blurred from.
///
/// Refuses (as `conflict`) a guide open in the editor; try again after it
/// closes.
#[tauri::command]
pub async fn sync_apply_guide(
    cell: State<'_, Arc<SessionCell>>,
    state: State<'_, Arc<SyncState>>,
    workspace_id: String,
    guide: Guide,
    assets: BTreeMap<String, String>,
) -> Result<PathBuf> {
    let cell = Arc::clone(&cell);
    let paths: Vec<String> = image_paths(&guide)
        .into_iter()
        .map(|(_, p)| p.to_owned())
        .collect();
    for path in &paths {
        check_image_path(path)?;
        if !assets.contains_key(path) {
            return Err(AppError::new(
                ErrorKind::Invalid,
                format!("no image for {path}"),
            ));
        }
    }

    let id = guide.id;
    let existing = {
        let cell = Arc::clone(&cell);
        blocking(move || Ok(cell.get()?.library().find(id)?)).await?
    };
    if let Some(handle) = &existing {
        // Refuse early, before downloading anything.
        if let Some(pid) = cell.get()?.library().editor_lock_holder(handle) {
            return Err(open_in_editor(EditError::OpenInEditor { id, pid }));
        }
    }

    // Images the bundle lacks, downloaded into memory first.
    let missing: Vec<String> = match &existing {
        Some(handle) => {
            let handle = handle.clone();
            let paths = paths.clone();
            blocking(move || {
                Ok(paths
                    .into_iter()
                    .filter(|p| handle.resolve(p).map(|f| !f.exists()).unwrap_or(false))
                    .collect())
            })
            .await?
        }
        None => paths.clone(),
    };
    let (server, token) = blocking(credentials).await?;
    let mut downloads = Vec::new();
    for path in missing {
        let sha = &assets[&path];
        let response = state
            .http
            .get(format!(
                "{server}/api/workspaces/{workspace_id}/assets/{sha}"
            ))
            .bearer_auth(token.expose())
            .send()
            .await
            .map_err(http_error)?;
        if !response.status().is_success() {
            return Err(AppError::new(
                ErrorKind::Io,
                format!("image download failed ({})", response.status()),
            ));
        }
        let bytes = response.bytes().await.map_err(http_error)?;
        if sha256_hex(&bytes) != *sha {
            return Err(AppError::new(
                ErrorKind::Io,
                format!("downloaded image {path} doesn't match its hash"),
            ));
        }
        downloads.push((path, bytes.to_vec()));
    }

    blocking(move || {
        let session = cell.get()?;
        let library = session.library();
        let handle = match existing {
            Some(handle) => {
                write_images(&handle, &downloads)?;
                library
                    .edit(id, |current| {
                        *current = guide;
                        Ok(())
                    })
                    .map_err(open_in_editor)?
                    .handle
            }
            None => {
                let handle = library.create(&guide)?;
                write_images(&handle, &downloads)?;
                handle
            }
        };
        session.index_bundle(&handle);
        Ok(handle.path)
    })
    .await
}

/// Images go in `steps/` or `assets/`, as the app writes them.
fn check_image_path(path: &str) -> Result<()> {
    let ok = (path.starts_with("steps/") || path.starts_with("assets/"))
        && Path::new(path)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)));
    if ok {
        Ok(())
    } else {
        Err(AppError::new(
            ErrorKind::Invalid,
            format!("unexpected image path {path}"),
        ))
    }
}

fn write_images(
    handle: &guiderecorder_core::library::GuideHandle,
    images: &[(String, Vec<u8>)],
) -> Result<()> {
    for (path, bytes) in images {
        let file = handle.resolve(path)?;
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir).map_err(io_error("cannot create the images folder"))?;
        }
        write_atomic(&file, bytes, Durability::RenameOnly)?;
    }
    Ok(())
}

fn open_in_editor(e: EditError) -> AppError {
    match e {
        EditError::OpenInEditor { .. } => {
            AppError::new(ErrorKind::Conflict, "the guide is open in the editor")
        }
        EditError::Library(e) => e.into(),
        other => AppError::new(ErrorKind::Invalid, other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servers_must_be_https_or_local() {
        assert_eq!(
            normalize_server("https://app.example.com/dashboard").unwrap(),
            "https://app.example.com"
        );
        assert_eq!(
            normalize_server(" http://localhost:3000 ").unwrap(),
            "http://localhost:3000"
        );
        assert!(normalize_server("http://app.example.com").is_err());
        assert!(normalize_server("ftp://app.example.com").is_err());
        assert!(normalize_server("https://user@app.example.com").is_err());
        assert!(normalize_server("not a url").is_err());
    }

    #[test]
    fn accounts_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_account(dir.path()), SyncAccount::default());
        let account = SyncAccount {
            server: Some("https://a.example".into()),
            signed_in: true,
        };
        write_account(dir.path(), &account).unwrap();
        assert_eq!(read_account(dir.path()), account);
    }

    #[test]
    fn pulled_images_stay_in_their_folders() {
        assert!(check_image_path("steps/01M4WZCE2000000000000002R2.webp").is_ok());
        assert!(check_image_path("assets/drucker-fach.webp").is_ok());
        assert!(check_image_path("../escape.webp").is_err());
        assert!(check_image_path("steps/../../escape.webp").is_err());
        assert!(check_image_path("guide.json").is_err());
        assert!(check_image_path("/etc/passwd").is_err());
    }
}
