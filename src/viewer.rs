use crate::mime::{ParsedMessage, normalize_cid};
use anyhow::Result;
use axum::{
    Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use rand::RngCore;
use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};

pub const CSP: &str = "default-src 'none'; script-src 'none'; connect-src 'none'; form-action 'none'; frame-src 'none'; frame-ancestors 'none'; object-src 'none'; base-uri 'none'; img-src 'self'; style-src 'unsafe-inline'; font-src 'none'; sandbox allow-popups allow-popups-to-escape-sandbox";
const MAX_SESSIONS: usize = 8;

struct Session {
    expires: Instant,
    html: String,
    remote_html: String,
    assets: Vec<crate::mime::Asset>,
}
#[derive(Clone)]
struct ViewerState {
    host: String,
    sessions: Arc<Mutex<HashMap<String, Session>>>,
}

pub struct Viewer {
    state: ViewerState,
    ttl: Duration,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

pub fn sanitize(message: &ParsedMessage, token: &str) -> String {
    sanitize_remote(message, token, false)
}
fn sanitize_remote(message: &ParsedMessage, token: &str, remote: bool) -> String {
    let assets: HashMap<_, _> = message
        .assets
        .iter()
        .enumerate()
        .map(|(i, a)| (a.cid.clone(), format!("/v/{token}/cid/{i}")))
        .collect();
    let mut cleaner = ammonia::Builder::default();
    cleaner
        .url_schemes(["http", "https", "cid"].into_iter().collect())
        .link_rel(Some("noopener noreferrer"))
        .attribute_filter(move |element, attr, value| {
            if element == "img" && attr == "src" {
                if remote && (value.starts_with("https://") || value.starts_with("http://")) {
                    return Some(Cow::Owned(value.to_string()));
                }
                return value
                    .strip_prefix("cid:")
                    .and_then(|cid| assets.get(&normalize_cid(cid)))
                    .map(|s| Cow::Owned(s.clone()));
            }
            if attr == "href" && !(value.starts_with("https://") || value.starts_with("http://")) {
                return None;
            }
            Some(Cow::Owned(value.to_string()))
        });
    let source = message
        .html
        .clone()
        .unwrap_or_else(|| format!("<pre>{}</pre>", escape(&message.text)));
    let safe = cleaner.clean(&source).to_string();
    let button = if remote {
        format!(
            "Images distantes activées · <a role=\"button\" href=\"/v/{token}/message\">Masquer les images distantes</a>"
        )
    } else {
        format!(
            "Images distantes bloquées · <a role=\"button\" href=\"/v/{token}/message/remote\">Afficher les images distantes</a>"
        )
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>MailTUI · private viewer</title><style>body{{font:16px/1.6 system-ui;max-width:960px;margin:2rem auto;padding:0 1rem}}img{{max-width:100%}}pre{{white-space:pre-wrap}}.notice{{border-bottom:1px solid #888;color:#666}}a[role=button]{{display:inline-block;padding:.3rem .7rem;border:1px solid #888;border-radius:4px}}</style></head><body><p class=\"notice\">MailTUI · sanitized message · {button}</p>{safe}</body></html>"
    )
}
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

impl Viewer {
    pub async fn start(ttl: Duration) -> Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let state = ViewerState {
            host: listener.local_addr()?.to_string(),
            sessions: Default::default(),
        };
        let app = routes(state.clone());
        let (shutdown, receive) = oneshot::channel();
        let cleanup = state.clone();
        let task = tokio::spawn(async move {
            let serve = async {
                axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = receive.await;
                    })
                    .await
            };
            tokio::pin!(serve);
            // Prune even if no more messages are opened or requested.
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    _ = &mut serve => break,
                    _ = tick.tick() => cleanup.sessions.lock().unwrap().retain(|_,s| s.expires > Instant::now()),
                }
            }
        });
        Ok(Self {
            state,
            ttl,
            shutdown: Some(shutdown),
            task,
        })
    }
    pub fn session(&self, message: &ParsedMessage) -> String {
        let mut entropy = [0u8; 32];
        rand::rng().fill_bytes(&mut entropy);
        let token: String = entropy.iter().map(|b| format!("{b:02x}")).collect();
        let mut sessions = self.state.sessions.lock().unwrap();
        sessions.retain(|_, s| s.expires > Instant::now());
        if sessions.len() >= MAX_SESSIONS
            && let Some(old) = sessions
                .iter()
                .min_by_key(|(_, s)| s.expires)
                .map(|(t, _)| t.clone())
        {
            sessions.remove(&old);
        }
        sessions.insert(
            token.clone(),
            Session {
                expires: Instant::now() + self.ttl,
                html: sanitize(message, &token),
                remote_html: sanitize_remote(message, &token, true),
                assets: message.assets.clone(),
            },
        );
        format!("http://{}/v/{token}/message", self.state.host)
    }
    pub fn session_with_mode(&self, message: &ParsedMessage, remote: bool) -> String {
        let url = self.session(message);
        if remote { format!("{url}/remote") } else { url }
    }
    pub fn revoke(&self) {
        self.state.sessions.lock().unwrap().clear();
    }
    pub async fn stop(mut self) {
        self.revoke();
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        let _ = (&mut self.task).await;
    }
}
impl Drop for Viewer {
    fn drop(&mut self) {
        self.revoke();
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        self.task.abort();
    }
}

fn routes(state: ViewerState) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/v/{token}/message", get(document))
        .route("/v/{token}/message/remote", get(remote_document))
        .route("/v/{token}/cid/{asset}", get(asset))
        .with_state(state)
}
fn response(status: StatusCode, mime: &str, bytes: Vec<u8>) -> Response {
    (
        status,
        [
            ("content-type", mime),
            ("cache-control", "no-store"),
            ("referrer-policy", "no-referrer"),
            ("x-content-type-options", "nosniff"),
            ("content-security-policy", CSP),
            ("content-disposition", "inline"),
        ],
        bytes,
    )
        .into_response()
}
fn check_host(headers: &HeaderMap, state: &ViewerState) -> bool {
    headers.get("host").and_then(|h| h.to_str().ok()) == Some(state.host.as_str())
}
async fn document(
    State(state): State<ViewerState>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !check_host(&headers, &state) {
        return response(StatusCode::FORBIDDEN, "text/plain", vec![]);
    }
    let sessions = state.sessions.lock().unwrap();
    match sessions.get(&token).filter(|s| s.expires > Instant::now()) {
        Some(s) => response(
            StatusCode::OK,
            "text/html; charset=utf-8",
            s.html.as_bytes().to_vec(),
        ),
        None => response(
            StatusCode::NOT_FOUND,
            "text/plain",
            b"Viewer session unavailable or expired".to_vec(),
        ),
    }
}
async fn remote_document(
    State(state): State<ViewerState>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !check_host(&headers, &state) {
        return response(StatusCode::FORBIDDEN, "text/plain", vec![]);
    }
    let sessions = state.sessions.lock().unwrap();
    match sessions.get(&token).filter(|s| s.expires > Instant::now()) {
        Some(session) => {
            let mut response = response(
                StatusCode::OK,
                "text/html; charset=utf-8",
                session.remote_html.as_bytes().to_vec(),
            );
            response.headers_mut().insert(
                "content-security-policy",
                CSP.replace("img-src 'self'", "img-src 'self' https: http:")
                    .parse()
                    .unwrap(),
            );
            response
        }
        None => response(StatusCode::NOT_FOUND, "text/plain", vec![]),
    }
}
async fn asset(
    State(state): State<ViewerState>,
    Path((token, id)): Path<(String, usize)>,
    headers: HeaderMap,
) -> Response {
    if !check_host(&headers, &state) {
        return response(StatusCode::FORBIDDEN, "text/plain", vec![]);
    }
    let sessions = state.sessions.lock().unwrap();
    match sessions
        .get(&token)
        .filter(|s| s.expires > Instant::now())
        .and_then(|s| s.assets.get(id))
    {
        Some(a) => response(StatusCode::OK, &a.content_type, a.bytes.clone()),
        None => response(StatusCode::NOT_FOUND, "text/plain", vec![]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn actual_loopback_http_and_revocation() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let viewer = Viewer::start(Duration::from_secs(600)).await.unwrap();
        let message = crate::mime::parse(include_bytes!("../tests/fixtures/related.eml")).unwrap();
        let url = viewer.session(&message);
        assert!(!url.ends_with("/remote"));
        assert!(
            viewer
                .session_with_mode(&message, true)
                .ends_with("/message/remote")
        );
        let path = url
            .strip_prefix(&format!("http://{}", viewer.state.host))
            .unwrap();
        let mut stream = tokio::net::TcpStream::connect(&viewer.state.host)
            .await
            .unwrap();
        stream
            .write_all(
                format!(
                    "GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                    viewer.state.host
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = vec![];
        stream.read_to_end(&mut response).await.unwrap();
        let response = String::from_utf8(response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("cache-control: no-store"));
        assert!(response.contains("Release planning"));
        for _ in 0..10 {
            viewer.session(&message);
        }
        assert_eq!(viewer.state.sessions.lock().unwrap().len(), MAX_SESSIONS);
        viewer.revoke();
        assert!(viewer.state.sessions.lock().unwrap().is_empty());
        viewer.stop().await;
    }
    #[tokio::test]
    async fn remote_images_require_an_explicit_authenticated_navigation() {
        let viewer = Viewer::start(Duration::from_secs(600)).await.unwrap();
        let mut message =
            crate::mime::parse(include_bytes!("../tests/fixtures/related.eml")).unwrap();
        message.html = Some("<img src=\"https://tracker.example/image.png\"><script>alert(1)</script><iframe src=\"https://tracker.example/frame\"></iframe>".into());
        let url = viewer.session(&message);
        let path = url
            .strip_prefix(&format!("http://{}", viewer.state.host))
            .unwrap();
        let app = routes(viewer.state.clone());
        let request = |path: &str| {
            Request::builder()
                .uri(path)
                .header("host", &viewer.state.host)
                .body(Body::empty())
                .unwrap()
        };
        let response = app.clone().oneshot(request(path)).await.unwrap();
        assert_eq!(response.headers()["content-security-policy"], CSP);
        let html = String::from_utf8(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        assert!(html.contains("Afficher les images distantes"));
        assert!(!html.contains("tracker.example"));
        let remote = format!("{path}/remote");
        let response = app.clone().oneshot(request(&remote)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers()["content-security-policy"]
                .to_str()
                .unwrap()
                .contains("img-src 'self' https: http:")
        );
        let html = String::from_utf8(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        assert!(html.contains("https://tracker.example/image.png"));
        assert!(html.contains("Masquer les images distantes"));
        assert!(!html.contains("<script") && !html.contains("<iframe"));
        viewer.revoke();
        assert_eq!(
            app.oneshot(request(&remote)).await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
        viewer.stop().await;
    }
    #[tokio::test]
    async fn token_host_expiry_and_cid_routes() {
        let viewer = Viewer::start(Duration::from_secs(600)).await.unwrap();
        assert!(viewer.state.host.starts_with("127.0.0.1:"));
        let message = crate::mime::parse(include_bytes!("../tests/fixtures/related.eml")).unwrap();
        let url = viewer.session(&message);
        let path = url
            .strip_prefix(&format!("http://{}", viewer.state.host))
            .unwrap();
        let app = routes(viewer.state.clone());
        let request = |p: &str, host: &str| {
            Request::builder()
                .uri(p)
                .header("host", host)
                .body(Body::empty())
                .unwrap()
        };
        let res = app
            .clone()
            .oneshot(request(path, &viewer.state.host))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()["cache-control"], "no-store");
        assert_eq!(res.headers()["content-security-policy"], CSP);
        let body = String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec())
            .unwrap();
        assert!(body.contains(&path.replace("message", "cid/0")), "{body}");
        assert!(!body.contains("tracker.invalid"));
        assert!(!body.contains("<script"));
        assert_eq!(
            app.clone()
                .oneshot(request(
                    &path.replace("message", "cid/0"),
                    &viewer.state.host
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            app.clone()
                .oneshot(request(path, "evil.invalid"))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            app.clone()
                .oneshot(request("/v/wrong/message", &viewer.state.host))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        for session in viewer.state.sessions.lock().unwrap().values_mut() {
            session.expires = Instant::now();
        }
        assert_eq!(
            app.oneshot(request(path, &viewer.state.host))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        viewer.stop().await;
    }
    #[test]
    fn strips_active_html_and_remote_resources() {
        let p = crate::mime::parse(b"Content-Type: text/html\r\n\r\n<form><input></form><svg onload='bad()'></svg><iframe src='https://evil.invalid'></iframe><meta http-equiv='refresh' content='0;url=https://evil.invalid'><p style='background:url(https://tracker.invalid)' onclick='bad()'>Hello</p><img src='https://tracker.invalid'><a href='javascript:bad()'>bad</a>").unwrap();
        let safe = sanitize(&p, "token");
        for bad in [
            "<form",
            "<svg",
            "<iframe",
            "<input",
            "onclick",
            "javascript:",
            "tracker.invalid",
            "http-equiv",
        ] {
            assert!(!safe.contains(bad), "{bad}: {safe}");
        }
    }
}
