//! Minimal HTTP/1.1 server and client for the market services and the wallet's local proxy
//! (m5-gateway-provider design D1): hyper 1.x without a web framework. Responses can stream;
//! nothing here logs request or response bodies.

use std::convert::Infallible;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Body as HttpBody, Frame, Incoming};
use hyper::header::{CONTENT_TYPE, HeaderValue};
use hyper::service::service_fn;
use hyper_util::client::legacy::Client as LegacyClient;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// A request as received by a server.
pub type Request = hyper::Request<Incoming>;

/// A response body: either complete or streamed through a [`BodySender`].
pub struct Body(Inner);

enum Inner {
    Full(Option<Bytes>),
    Channel(mpsc::Receiver<Bytes>),
}

impl HttpBody for Body {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        match &mut self.0 {
            Inner::Full(bytes) => Poll::Ready(bytes.take().map(|b| Ok(Frame::data(b)))),
            Inner::Channel(rx) => rx.poll_recv(cx).map(|b| b.map(|b| Ok(Frame::data(b)))),
        }
    }
}

/// A response.
pub type Response = hyper::Response<Body>;

/// Feeds a streamed response; dropping it ends the body.
#[derive(Clone, Debug)]
pub struct BodySender(mpsc::Sender<Bytes>);

impl BodySender {
    /// Sends bytes; `false` once the peer has gone away.
    pub async fn send(&self, bytes: impl Into<Bytes>) -> bool {
        self.0.send(bytes.into()).await.is_ok()
    }
}

/// A complete response.
#[must_use]
pub fn full(status: u16, content_type: &str, body: impl Into<Bytes>) -> Response {
    let mut r = hyper::Response::new(Body(Inner::Full(Some(body.into()))));
    *r.status_mut() =
        hyper::StatusCode::from_u16(status).unwrap_or(hyper::StatusCode::INTERNAL_SERVER_ERROR);
    if let Ok(v) = HeaderValue::from_str(content_type) {
        r.headers_mut().insert(CONTENT_TYPE, v);
    }
    r
}

/// A JSON response.
#[must_use]
pub fn json(status: u16, body: impl Into<Bytes>) -> Response {
    full(status, "application/json", body)
}

/// A streamed response and the sender that feeds it.
#[must_use]
pub fn streaming(status: u16, content_type: &str) -> (BodySender, Response) {
    let (tx, rx) = mpsc::channel(64);
    let mut r = hyper::Response::new(Body(Inner::Channel(rx)));
    *r.status_mut() = hyper::StatusCode::from_u16(status).unwrap_or(hyper::StatusCode::OK);
    if let Ok(v) = HeaderValue::from_str(content_type) {
        r.headers_mut().insert(CONTENT_TYPE, v);
    }
    (BodySender(tx), r)
}

/// Reads a whole request body, at most `limit` bytes.
///
/// # Errors
///
/// Transport errors or a body over the limit.
pub async fn read_body(body: Incoming, limit: usize) -> Result<Bytes> {
    Ok(Limited::new(body, limit)
        .collect()
        .await
        .map_err(|e| anyhow::anyhow!("reading the body: {e}"))?
        .to_bytes())
}

/// The next data chunk of a streamed body, or `None` at its end.
///
/// # Errors
///
/// Transport errors.
pub async fn next_chunk(body: &mut Incoming) -> Result<Option<Bytes>> {
    loop {
        match body.frame().await {
            None => return Ok(None),
            Some(frame) => {
                if let Ok(data) = frame.context("reading the body")?.into_data() {
                    return Ok(Some(data));
                }
            }
        }
    }
}

/// Serves HTTP/1.1 on `listener` until the process ends, one task per connection.
///
/// # Errors
///
/// Accept failures.
pub async fn serve<H, F>(listener: TcpListener, handler: H) -> io::Result<()>
where
    H: Fn(Request) -> F + Clone + Send + Sync + 'static,
    F: Future<Output = Response> + Send + 'static,
{
    loop {
        let (stream, _) = listener.accept().await?;
        let handler = handler.clone();
        tokio::spawn(async move {
            let service = service_fn(move |req| {
                let handler = handler.clone();
                async move { Ok::<_, Infallible>(handler(req).await) }
            });
            // Connection errors concern one client only; there is nothing to report.
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
    }
}

/// An HTTP(S) client (native root certificates for `https://`).
#[derive(Clone)]
pub struct Client {
    inner: LegacyClient<hyper_rustls::HttpsConnector<HttpConnector>, Full<Bytes>>,
}

impl core::fmt::Debug for Client {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Client")
    }
}

impl Client {
    /// A client for `http://` and `https://` URLs.
    ///
    /// # Errors
    ///
    /// If the platform's root certificates cannot be loaded.
    pub fn new() -> Result<Self> {
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_native_roots()
            .context("loading root certificates")?
            .https_or_http()
            .enable_http1()
            .build();
        Ok(Self {
            inner: LegacyClient::builder(TokioExecutor::new()).build(https),
        })
    }

    /// `GET url`.
    ///
    /// # Errors
    ///
    /// Invalid URLs and transport errors.
    pub async fn get(&self, url: &str) -> Result<hyper::Response<Incoming>> {
        let req = hyper::Request::get(url)
            .body(Full::new(Bytes::new()))
            .context("building the request")?;
        self.inner
            .request(req)
            .await
            .with_context(|| format!("GET {url}"))
    }

    /// `POST url` with a complete body.
    ///
    /// # Errors
    ///
    /// Invalid URLs and transport errors.
    pub async fn post(
        &self,
        url: &str,
        content_type: &str,
        body: impl Into<Bytes>,
    ) -> Result<hyper::Response<Incoming>> {
        let req = hyper::Request::post(url)
            .header(CONTENT_TYPE, content_type)
            .body(Full::new(body.into()))
            .context("building the request")?;
        self.inner
            .request(req)
            .await
            .with_context(|| format!("POST {url}"))
    }

    /// `GET url` and the whole body, which must be a 2xx response of at most `limit` bytes.
    ///
    /// # Errors
    ///
    /// Transport errors, non-2xx statuses and oversize bodies.
    pub async fn get_bytes(&self, url: &str, limit: usize) -> Result<Bytes> {
        let resp = self.get(url).await?;
        let status = resp.status();
        let body = read_body(resp.into_body(), limit).await?;
        if !status.is_success() {
            bail!("GET {url}: HTTP {status}");
        }
        Ok(body)
    }
}

/// Joins a base URL and a path without doubling the slash.
#[must_use]
pub fn join(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn serve_full_and_streamed_responses() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(serve(listener, |req: Request| async move {
            if req.uri().path() == "/stream" {
                let (tx, resp) = streaming(200, "text/plain");
                tokio::spawn(async move {
                    for part in ["a", "b", "c"] {
                        tx.send(part).await;
                    }
                });
                resp
            } else {
                let body = read_body(req.into_body(), 16).await.unwrap_or_default();
                json(201, body)
            }
        }));
        let client = Client::new().unwrap();
        let resp = client
            .post(&join(&base, "/echo"), "application/json", "{}")
            .await
            .unwrap();
        assert_eq!(resp.status(), 201);
        assert_eq!(read_body(resp.into_body(), 16).await.unwrap(), "{}");
        let mut body = client
            .get(&join(&base, "stream"))
            .await
            .unwrap()
            .into_body();
        let mut got = Vec::new();
        while let Some(chunk) = next_chunk(&mut body).await.unwrap() {
            got.extend_from_slice(&chunk);
        }
        assert_eq!(got, b"abc");
        // A body over the limit is refused.
        assert!(
            client
                .post(&join(&base, "/echo"), "text/plain", vec![0u8; 64])
                .await
                .is_ok()
        );
        assert!(client.get_bytes(&join(&base, "/echo"), 1).await.is_ok());
    }
}
