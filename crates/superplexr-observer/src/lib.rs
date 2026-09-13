//! Optional scoped browser projection. Read-only unless explicitly enabled.
mod control;
mod display;
mod embedding;
mod search;
mod session_feed;
mod workflow;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{
        Html, IntoResponse, Response, Sse,
        sse::{Event, KeepAlive},
    },
    routing::{get, post},
};
pub use embedding::EmbeddingPolicy;
use serde::Serialize;
use std::{convert::Infallible, sync::Arc};
use superplexr_client::{ClientError, ControlClient, EventSubscription, TerminalStreamUpdate};
use superplexr_core::SessionId;
use superplexr_protocol::{ServerEvent, TerminalSessionStatus};
use superplexr_terminal::FullFrame;
use tokio::sync::{Semaphore, mpsc};
use tokio_stream::wrappers::ReceiverStream;

#[derive(Clone)]
pub struct Observer {
    client: ControlClient,
    authority: Arc<str>,
    origin: Arc<str>,
    authorization: Arc<str>,
    streams: Arc<Semaphore>,
    shutdown: tokio::sync::watch::Sender<bool>,
    allow_control: bool,
    workflow_read: bool,
    views: Arc<control::Views>,
    commands: Arc<Semaphore>,
    searches: Arc<Semaphore>,
    embedding: Option<Arc<EmbeddingPolicy>>,
}

impl Observer {
    /// Requires an already scoped Observer Share. Owner and Controller clients
    /// are rejected even though the HTTP routes themselves contain no writes.
    pub fn new(
        client: ControlClient,
        authority: String,
        access_key: String,
    ) -> Result<Self, &'static str> {
        if !client.is_observer() {
            return Err("browser gateway requires an Observer Share");
        }
        Self::configured(client, authority, access_key, false)
    }

    /// Explicit interactive mode accepts only Controller Shares, never owners.
    pub fn with_control(
        client: ControlClient,
        authority: String,
        access_key: String,
    ) -> Result<Self, &'static str> {
        if !client.is_shared() || client.is_observer() {
            return Err("interactive browser gateway requires a Controller Share");
        }
        Self::configured(client, authority, access_key, true)
    }

    fn configured(
        client: ControlClient,
        authority: String,
        access_key: String,
        allow_control: bool,
    ) -> Result<Self, &'static str> {
        if access_key.len() < 32 {
            return Err("browser access key must contain at least 32 characters");
        }
        Ok(Self {
            client,
            origin: format!("http://{authority}").into(),
            authority: authority.into(),
            authorization: format!("Bearer {access_key}").into(),
            streams: Arc::new(Semaphore::new(8)),
            shutdown: tokio::sync::watch::channel(false).0,
            allow_control,
            workflow_read: false,
            views: Arc::new(control::Views::default()),
            commands: Arc::new(Semaphore::new(4)),
            searches: Arc::new(Semaphore::new(4)),
            embedding: None,
        })
    }

    /// Allow trusted origins to frame the browser UI. This does not allow
    /// cross-origin API access, change the Share, or enable terminal Control.
    /// A Controller gateway additionally requires explicit acknowledgement of
    /// the parent application's ability to disguise or overlay the input UI.
    pub fn with_embedding(
        mut self,
        policy: EmbeddingPolicy,
        allow_embedded_control: bool,
    ) -> Result<Self, &'static str> {
        if self.allow_control && !allow_embedded_control {
            return Err("embedding a Controller gateway requires explicit embedded-control opt-in");
        }
        self.embedding = Some(Arc::new(policy));
        Ok(self)
    }

    /// Enable only read-only workflow inspection; Share scope still applies.
    pub fn with_workflow_inspection(mut self) -> Self {
        self.workflow_read = true;
        self
    }

    /// Close active observation streams without touching runtime processes.
    pub fn shutdown(&self) {
        self.shutdown.send_replace(true);
    }

    pub fn router(self) -> Router {
        Router::new()
            .route(
                "/",
                get(|| async { Html(include_str!("../web/index.html")) }),
            )
            .route(
                "/observer.js",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                        include_str!("../web/observer.js"),
                    )
                }),
            )
            .route(
                "/observer.css",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                        include_str!("../web/observer.css"),
                    )
                }),
            )
            .route("/sessions", get(sessions))
            .route("/sessions/events", get(session_feed::events))
            .route(
                "/missions/{mission}/verifiers/{verifier}/status",
                get(workflow::status),
            )
            .route("/missions/{mission}/verifiers", get(workflow::catalog))
            .route(
                "/workflow.mjs",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                        include_str!("../web/workflow.mjs"),
                    )
                }),
            )
            .route(
                "/side-views.mjs",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                        include_str!("../web/side-views.mjs"),
                    )
                }),
            )
            .route(
                "/frames.mjs",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                        include_str!("../web/frames.mjs"),
                    )
                }),
            )
            .route(
                "/terminal.mjs",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                        include_str!("../web/terminal.mjs"),
                    )
                }),
            )
            .route(
                "/sessions.mjs",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                        include_str!("../web/sessions.mjs"),
                    )
                }),
            )
            .route(
                "/stream.mjs",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                        include_str!("../web/stream.mjs"),
                    )
                }),
            )
            .route("/sessions/{id}/events", get(events))
            .route("/sessions/{id}/history/{offset}", get(history))
            .route(
                "/sessions/{id}/history-line/{row}",
                get(search::history_line),
            )
            .route("/sessions/{id}/search", post(search::start))
            .route(
                "/search.mjs",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                        include_str!("../web/search.mjs"),
                    )
                }),
            )
            .route("/views/{id}/commands", post(control::command))
            .route(
                "/control.mjs",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                        include_str!("../web/control.mjs"),
                    )
                }),
            )
            .layer(DefaultBodyLimit::max(96 * 1024))
            .layer(middleware::from_fn_with_state(self.clone(), guard))
            .with_state(self)
    }
}

fn same_key(actual: &[u8], expected: &[u8]) -> bool {
    actual.len() == expected.len()
        && actual
            .iter()
            .zip(expected)
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

async fn guard(State(state): State<Observer>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    // Only a document navigation gets the framing exception. Parent-page
    // fetches, scripts, streams, searches, and commands retain the original
    // same-origin policy. CSP checks every ancestor, including nested frames.
    let frame_navigation = state.embedding.is_some()
        && request.method() == axum::http::Method::GET
        && request.uri().path() == "/"
        && headers
            .get("sec-fetch-mode")
            .is_some_and(|v| v == "navigate")
        && headers.get("sec-fetch-dest").is_some_and(|v| v == "iframe");
    let host_ok = headers
        .get(header::HOST)
        .is_some_and(|host| host.as_bytes() == state.authority.as_bytes());
    let origin_ok = headers.get(header::ORIGIN).is_none_or(|origin| {
        origin.as_bytes() == state.origin.as_bytes()
            || (frame_navigation
                && state
                    .embedding
                    .as_ref()
                    .is_some_and(|policy| policy.permits_origin(origin)))
    });
    let site_ok = headers.get("sec-fetch-site").is_none_or(|site| {
        matches!(site.as_bytes(), b"same-origin" | b"none")
            || (frame_navigation && matches!(site.as_bytes(), b"same-site" | b"cross-site"))
    });
    let public = matches!(
        request.uri().path(),
        "/" | "/observer.js"
            | "/observer.css"
            | "/stream.mjs"
            | "/control.mjs"
            | "/search.mjs"
            | "/sessions.mjs"
            | "/terminal.mjs"
            | "/frames.mjs"
            | "/side-views.mjs"
            | "/workflow.mjs"
    );
    let authorized = public
        || headers
            .get(header::AUTHORIZATION)
            .is_some_and(|value| same_key(value.as_bytes(), state.authorization.as_bytes()));
    let write_origin_ok = request.method() != axum::http::Method::POST
        || headers
            .get(header::ORIGIN)
            .is_some_and(|origin| origin.as_bytes() == state.origin.as_bytes());
    let mut response = if !host_ok || !origin_ok || !site_ok || !write_origin_ok {
        StatusCode::FORBIDDEN.into_response()
    } else if !authorized {
        StatusCode::UNAUTHORIZED.into_response()
    } else {
        next.run(request).await
    };
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    if let Some(policy) = &state.embedding {
        // X-Frame-Options cannot represent an origin list; CSP is authoritative.
        headers.remove("x-frame-options");
        headers.insert("content-security-policy", policy.csp.clone());
    } else {
        headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
        headers.insert(
            "content-security-policy",
            HeaderValue::from_static(embedding::STANDALONE_CSP),
        );
    }
    response
}

async fn sessions(State(state): State<Observer>) -> Result<Json<serde_json::Value>, StatusCode> {
    let permit = state
        .streams
        .clone()
        .try_acquire_owned()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let terminals = state.client.list_terminals().map_err(client_status)?;
        Ok(Json(
            serde_json::json!({ "terminals": terminals, "allow_control": state.allow_control }),
        ))
    })
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
}

#[derive(Clone, Serialize)]
struct Snapshot {
    sequence: u64,
    revision: String,
    #[serde(skip)]
    continuity: u64,
    title: Option<String>,
    directory: Option<String>,
    text: String,
    status: superplexr_protocol::TerminalSessionStatus,
    rows: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    display: Option<display::Display>,
}

impl Snapshot {
    fn from_frame(frame: &FullFrame, status: TerminalSessionStatus) -> Self {
        Self {
            sequence: frame.sequence,
            revision: frame.sequence.to_string(),
            continuity: 0,
            title: frame.title.clone(),
            directory: frame.current_directory.clone(),
            text: frame
                .rows
                .iter()
                .map(|row| row.text())
                .collect::<Vec<_>>()
                .join("\n"),
            status,
            rows: frame.grid.rows,
            display: display::Display::from_frame(frame),
        }
    }
}

fn client_status(error: ClientError) -> StatusCode {
    match error {
        ClientError::ReceiveLimit { .. } => StatusCode::PAYLOAD_TOO_LARGE,
        ClientError::Remote { .. } => StatusCode::FORBIDDEN,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    }
}

async fn history(
    State(state): State<Observer>,
    Path((id, offset)): Path<(SessionId, u32)>,
) -> Result<Json<Snapshot>, StatusCode> {
    if offset > 100_000 {
        return Err(StatusCode::BAD_REQUEST);
    }
    let permit = state
        .streams
        .clone()
        .try_acquire_owned()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let terminal = state.client.terminal(id);
        let status = terminal.capture().map_err(client_status)?.terminal.status;
        let frame = terminal.history_frame(offset).map_err(client_status)?;
        Ok(Json(Snapshot::from_frame(&frame, status)))
    })
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
}

#[derive(Clone)]
enum FeedState {
    Frame(Arc<Snapshot>),
    Reconnecting,
    Rejected,
    ReceiveLimited,
    Complete(Arc<Snapshot>),
}

/// Explicitly cancel on HTTP detach, even if the terminal is currently idle.
struct FeedSubscription(EventSubscription);
impl Drop for FeedSubscription {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

async fn events(
    State(state): State<Observer>,
    Path(id): Path<SessionId>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let row_deltas = headers
        .get("x-superplexr-frames")
        .is_some_and(|value| value.as_bytes() == b"row-delta-v1");
    let permit = state
        .streams
        .clone()
        .try_acquire_owned()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
    let client = state.client.clone();
    let views = state.views.clone();
    // Per-attachment downscoping: an observing side view gets no input Surface,
    // no Control lease and no input heartbeat, even on a Controller gateway.
    let allow_control = state.allow_control
        && !headers
            .get("x-superplexr-view")
            .is_some_and(|value| value.as_bytes() == b"observe");
    let (mut updates, subscription, surface, permit) = tokio::task::spawn_blocking(move || {
        // HTTP cancellation must not release admission while setup still runs.
        let mut permit = Some(permit);
        let terminal = client.terminal(id);
        let initial = terminal.capture().map_err(client_status)?;
        let surface = if allow_control {
            Some(views.insert(terminal.clone())?)
        } else {
            None
        };
        let view_guard = surface.clone().map(|surface| control::ViewGuard {
            surface,
            views,
            permit: permit.take(),
        });
        let mut latest = Arc::new(Snapshot::from_frame(
            &initial.frame,
            initial.terminal.status,
        ));
        let (publish, updates) = tokio::sync::watch::channel(FeedState::Frame(latest.clone()));
        let mut reconnecting = false;
        let mut continuity = 0u64;
        let live_surface = surface.clone();
        let subscription = terminal
            .subscribe_frames_with(move |update| {
                if publish.is_closed() {
                    return false;
                }
                let next = match update {
                    TerminalStreamUpdate::Event(ServerEvent::TerminalFrame { frame, .. }) => {
                        if frame.sequence == latest.sequence && !reconnecting {
                            return true;
                        }
                        reconnecting = false;
                        let mut snapshot = Snapshot::from_frame(&frame, latest.status);
                        snapshot.continuity = continuity;
                        latest = Arc::new(snapshot);
                        FeedState::Frame(latest.clone())
                    }
                    TerminalStreamUpdate::Event(ServerEvent::TerminalExited { .. }) => {
                        if let Some(surface) = &live_surface {
                            surface.invalidate();
                        }
                        Arc::make_mut(&mut latest).status = TerminalSessionStatus::Exited;
                        FeedState::Complete(latest.clone())
                    }
                    TerminalStreamUpdate::Event(ServerEvent::TerminalFailed { .. }) => {
                        if let Some(surface) = &live_surface {
                            surface.invalidate();
                        }
                        Arc::make_mut(&mut latest).status = TerminalSessionStatus::Failed;
                        FeedState::Complete(latest.clone())
                    }
                    TerminalStreamUpdate::Reconnecting => {
                        continuity = continuity.saturating_add(1);
                        if let Some(surface) = &live_surface {
                            surface.invalidate();
                        }
                        reconnecting = true;
                        FeedState::Reconnecting
                    }
                    TerminalStreamUpdate::Rejected => {
                        if let Some(surface) = &live_surface {
                            surface.invalidate();
                        }
                        FeedState::Rejected
                    }
                    TerminalStreamUpdate::ReceiveLimited => {
                        // Publish the terminal reason before invalidating the
                        // HTTP surface. A heartbeat must not turn this into a
                        // generic EOF which the browser would retry.
                        publish.send_replace(FeedState::ReceiveLimited);
                        if let Some(surface) = &live_surface {
                            surface.invalidate();
                        }
                        return false;
                    }
                    _ => return true,
                };
                publish.send_replace(next);
                true
            })
            .map_err(client_status)?;
        Ok::<_, StatusCode>((updates, FeedSubscription(subscription), view_guard, permit))
    })
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)??;
    // Latest-state coalescing: browser backpressure never blocks the native
    // decoder or builds an unbounded frame queue. Scrollback stays on the host.
    let (sender, receiver) = mpsc::channel::<Result<Event, Infallible>>(1);
    let mut shutdown = state.shutdown.subscribe();
    tokio::spawn(async move {
        let _permit = permit;
        let _subscription = subscription;
        let _surface_guard = surface;
        if let Some(view) = &_surface_guard {
            let event = Event::default()
                .event("surface")
                .data(view.surface.id.to_string());
            tokio::select! {
                _ = shutdown.changed() => return,
                result = sender.send(Ok(event)) => if result.is_err() { return; }
            }
        }
        let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(2));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut previous_frame: Option<Arc<Snapshot>> = None;
        loop {
            // Wait before projecting/serializing the latest frame. Capacity
            // waits still observe detach, shutdown and authority expiry.
            let Some(slot) = frame_slot(
                &sender,
                &mut shutdown,
                &mut heartbeat,
                _surface_guard.as_ref(),
                &updates,
            )
            .await
            else {
                break;
            };
            let next = updates.borrow_and_update().clone();
            let complete = matches!(next, FeedState::Complete(_));
            let ended = matches!(next, FeedState::Rejected | FeedState::ReceiveLimited);
            let event = match next {
                FeedState::Frame(snapshot) | FeedState::Complete(snapshot) => {
                    let full = match serde_json::to_string(snapshot.as_ref()) {
                        Ok(full) => full,
                        Err(_) => break,
                    };
                    let patch = if row_deltas && !complete {
                        previous_frame
                            .as_ref()
                            .and_then(|previous| display::FramePatch::between(previous, &snapshot))
                            .and_then(|patch| serde_json::to_string(&patch).ok())
                            .filter(|patch| patch.len() < full.len())
                    } else {
                        None
                    };
                    let (kind, data) = match patch {
                        Some(patch) => ("frame-delta", patch),
                        None => ("frame", full),
                    };
                    let event = Event::default()
                        .event(kind)
                        .id(snapshot.revision.clone())
                        .data(data);
                    // Updated only after obtaining ordered HTTP capacity.
                    // Ending this attachment retires its entire delta base.
                    if row_deltas {
                        previous_frame = Some(snapshot);
                    }
                    event
                }
                FeedState::Reconnecting => {
                    previous_frame = None;
                    Event::default()
                        .event("reconnecting")
                        .data("Runtime connection lost; reattaching to the same session.")
                }
                FeedState::Rejected => Event::default()
                    .event("ended")
                    .data("Session access ended."),
                FeedState::ReceiveLimited => Event::default()
                    .event("ended")
                    .data(superplexr_client::RECEIVE_LIMIT_MESSAGE),
            };
            slot.send(Ok(event));
            if complete {
                if let Some(slot) = frame_slot(
                    &sender,
                    &mut shutdown,
                    &mut heartbeat,
                    _surface_guard.as_ref(),
                    &updates,
                )
                .await
                {
                    slot.send(Ok(Event::default()
                        .event("complete")
                        .data("Session ended; history remains available.")));
                }
                break;
            }
            if ended {
                break;
            }
            loop {
                tokio::select! {
                    _ = shutdown.changed() => return,
                    _ = sender.closed() => return,
                    result = updates.changed() => { if result.is_err() { return; } break; }
                    _ = heartbeat.tick(), if _surface_guard.is_some() => {
                        if retire_frame_surface(_surface_guard.as_ref(), &updates) { return; }
                    }
                }
            }
        }
    });
    Ok(Sse::new(ReceiverStream::new(receiver))
        .keep_alive(KeepAlive::default())
        .into_response())
}

// Preserve pending terminal reasons (especially permanent receive-limit errors)
// so a slow browser is not told to reconnect by a generic EOF instead. Such
// views are already input-retired by the native callback. Their one pending
// notification retains stream admission until delivery, disconnect or shutdown.
fn retire_frame_surface(
    view: Option<&control::ViewGuard>,
    updates: &tokio::sync::watch::Receiver<FeedState>,
) -> bool {
    let Some(view) = view else {
        return false;
    };
    if view.surface.available() && !view.surface.expired() {
        return false;
    }
    if matches!(
        *updates.borrow(),
        FeedState::Complete(_) | FeedState::Rejected | FeedState::ReceiveLimited
    ) {
        return false;
    }
    view.surface.invalidate();
    true
}

async fn frame_slot<'a>(
    sender: &'a mpsc::Sender<Result<Event, Infallible>>,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    heartbeat: &mut tokio::time::Interval,
    view: Option<&control::ViewGuard>,
    updates: &tokio::sync::watch::Receiver<FeedState>,
) -> Option<mpsc::Permit<'a, Result<Event, Infallible>>> {
    loop {
        if *shutdown.borrow() || retire_frame_surface(view, updates) {
            return None;
        }
        tokio::select! {
            biased;
            _ = shutdown.changed() => return None,
            _ = heartbeat.tick(), if view.is_some() => {},
            slot = sender.reserve() => {
                let slot = slot.ok()?;
                if *shutdown.borrow() || retire_frame_surface(view, updates) { return None; }
                return Some(slot);
            }
        }
    }
}
