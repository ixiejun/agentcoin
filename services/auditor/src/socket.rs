//! The receiving end of a verify-mode engine plugin (spec `market/engine-plugin` "复核模式",
//! "与提供者的本机协议"): listens on a local Unix socket, takes only verify-mode plugins and
//! collects, for each request the re-check expects, its token-row segments up to the end
//! marker.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ac_market_proto::engine::{
    ENGINE_PROTOCOL_VERSION, EngineMode, EngineMsg, EngineReader, HelloRefusal, answer_hello,
    market_request_id,
};
use ac_market_proto::toploc::MARKET_PARAMS;
use ac_toploc::Segment;
use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Notify;

use crate::logging::TARGET;

#[derive(Default)]
struct Pending {
    segments: Vec<Segment>,
    finished: bool,
}

#[derive(Default)]
struct State {
    pending: BTreeMap<[u8; 32], Pending>,
    connections: u64,
}

/// Collects the rows of the requests a re-check sends to its engine.
#[derive(Clone)]
pub struct Rows {
    state: Arc<Mutex<State>>,
    notify: Arc<Notify>,
}

impl Rows {
    /// Listens on `path` (a stale socket file is replaced; the new one is readable and
    /// writable by its owner only) and serves plugins in the background.
    ///
    /// # Errors
    ///
    /// If the socket cannot be created.
    pub fn listen(path: &Path) -> Result<Self> {
        if path.exists() {
            std::fs::remove_file(path).context("removing a stale socket")?;
        }
        let listener = UnixListener::bind(path).context("binding the re-check socket")?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        let rows = Self {
            state: Arc::new(Mutex::new(State::default())),
            notify: Arc::new(Notify::new()),
        };
        let r = rows.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let r = r.clone();
                tokio::spawn(async move { r.serve(stream).await });
            }
        });
        Ok(rows)
    }

    /// Plugins accepted so far.
    #[must_use]
    pub fn connections(&self) -> u64 {
        self.state.lock().map_or(0, |s| s.connections)
    }

    /// Starts collecting the rows of `request`.
    pub fn expect(&self, request: [u8; 32]) {
        if let Ok(mut s) = self.state.lock() {
            s.pending.insert(request, Pending::default());
        }
    }

    /// The rows of `request` once its end marker arrived, or `None` after `wait`; the request
    /// is forgotten either way.
    pub async fn take(&self, request: [u8; 32], wait: Duration) -> Option<Vec<Segment>> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let notified = self.notify.notified();
            {
                let mut s = self.state.lock().ok()?;
                if s.pending.get(&request).is_some_and(|p| p.finished) {
                    return s.pending.remove(&request).map(|p| p.segments);
                }
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                if let Ok(mut s) = self.state.lock() {
                    s.pending.remove(&request);
                }
                return None;
            }
        }
    }

    async fn serve(&self, mut stream: UnixStream) {
        let mut reader = EngineReader::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut greeted = false;
        loop {
            let n = match stream.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            };
            reader.push(buf.get(..n).unwrap_or_default());
            loop {
                let msg = match reader.next_msg() {
                    Ok(Some(m)) => m,
                    Ok(None) => break,
                    Err(_) => {
                        log::warn!(target: TARGET, "the re-check plugin sent a malformed frame; closing");
                        return;
                    }
                };
                if greeted {
                    self.accept(msg);
                    continue;
                }
                let topk = u16::try_from(MARKET_PARAMS.topk).unwrap_or(u16::MAX);
                match answer_hello(&msg, EngineMode::Verify, topk) {
                    Ok((welcome, hidden)) => {
                        let Ok(frame) = welcome.to_frame() else {
                            return;
                        };
                        if stream.write_all(&frame).await.is_err() {
                            return;
                        }
                        if let Ok(mut s) = self.state.lock() {
                            s.connections = s.connections.saturating_add(1);
                        }
                        log::info!(target: TARGET, "re-check plugin connected (hidden size {hidden})");
                        greeted = true;
                    }
                    Err((refusal, welcome)) => {
                        if let Some(frame) = welcome.and_then(|w| w.to_frame().ok()) {
                            let _ = stream.write_all(&frame).await;
                        }
                        match refusal {
                            HelloRefusal::Version(v) => log::warn!(
                                target: TARGET,
                                "the re-check plugin speaks protocol version {v}, this auditor {ENGINE_PROTOCOL_VERSION}"
                            ),
                            HelloRefusal::Mode(_) => log::warn!(
                                target: TARGET,
                                "re-check plugin rejected: mode (a plugin in prove mode is for providers)"
                            ),
                            _ => {}
                        }
                        return;
                    }
                }
            }
        }
    }

    fn accept(&self, msg: EngineMsg) {
        let Ok(mut s) = self.state.lock() else {
            return;
        };
        match msg {
            EngineMsg::Segment {
                request,
                phase,
                len,
                candidates,
            } => {
                if let Some(p) = market_request_id(&request)
                    .and_then(|id| s.pending.get_mut(&id))
                    .filter(|p| !p.finished)
                {
                    p.segments.push(Segment {
                        phase,
                        len,
                        candidates,
                    });
                }
            }
            EngineMsg::Finish { request } => {
                if let Some(p) = market_request_id(&request).and_then(|id| s.pending.get_mut(&id)) {
                    p.finished = true;
                    self.notify.notify_waiters();
                }
            }
            _ => {}
        }
    }
}
