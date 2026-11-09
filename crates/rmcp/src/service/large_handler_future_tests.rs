//! Exercise the production request-dispatch scopes on the ordinary Rust test stack.
//!
//! Keep the handler future unboxed here: boxing at the fixture boundary would
//! hide the regression in the service loop. No custom thread or runtime stack.

use std::{hint::black_box, sync::Arc, time::Duration};

use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;

use super::{
    ORIGINATING_PROTOCOL_VERSION, ORIGINATING_REQUEST, RequestContext, RoleClient, RoleServer,
    serve_directly_with_ct,
};
use crate::{
    ErrorData, ServerHandler,
    model::{
        ClientCapabilities, ClientJsonRpcMessage, Implementation, InitializeRequestParams,
        ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult,
        RequestId, ServerCapabilities, ServerConfig, ServerJsonRpcMessage, ServerResult,
    },
    transport::{IntoTransport, Transport},
};

const WAIT: Duration = Duration::from_secs(5);
// Deliberately inline across suspension, like a large application dispatcher.
// Confirm baseline-red/fixed-green with this exact value before landing.
const HANDLER_BYTES: usize = 128 * 1024;

struct LargeResourceServer {
    started: mpsc::UnboundedSender<(RequestId, CancellationToken)>,
    finished: mpsc::UnboundedSender<(RequestId, bool)>,
    release: Arc<Notify>,
}

fn assert_request_context(context: &RequestContext<RoleServer>) {
    // Concurrent requests deliberately select different supported versions.
    let expected_version = match context.id {
        RequestId::Number(10) => ProtocolVersion::V_2025_06_18,
        RequestId::Number(20) => ProtocolVersion::V_2025_11_25,
        ref other => panic!("unexpected request ID: {other:?}"),
    };
    assert_eq!(ORIGINATING_REQUEST.with(Clone::clone), context.id);
    assert_eq!(
        ORIGINATING_PROTOCOL_VERSION.with(Clone::clone),
        Some(expected_version.clone()),
    );
    assert_eq!(context.protocol_version(), Some(expected_version));
}

impl ServerHandler for LargeResourceServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_resources().build())
            .with_protocol_version(ProtocolVersion::V_2025_11_25)
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let inline_state = [0x5a_u8; HANDLER_BYTES];
        assert_request_context(&context);
        assert!(!context.ct.is_cancelled());
        self.started
            .send((context.id.clone(), context.ct.clone()))
            .unwrap();

        match request.uri.as_str() {
            "test://cancel" => context.ct.cancelled().await,
            "test://complete" => self.release.notified().await,
            other => panic!("unexpected resource URI: {other}"),
        }

        // The state and both task locals must survive a real suspension.
        assert_eq!(black_box(&inline_state)[HANDLER_BYTES - 1], 0x5a);
        assert_request_context(&context);
        self.finished
            .send((context.id.clone(), context.ct.is_cancelled()))
            .unwrap();
        Ok(ReadResourceResult::new(Vec::new()).into())
    }
}

fn read_request(id: i64, uri: &str, version: &str) -> ClientJsonRpcMessage {
    serde_json::from_value(serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "resources/read",
        "params": {
            "uri": uri,
            "_meta": { "io.modelcontextprotocol/protocolVersion": version }
        }
    }))
    .unwrap()
}

#[tokio::test]
async fn large_resource_future_preserves_context_and_cancellation() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let (started_tx, mut started_rx) = mpsc::unbounded_channel();
    let (finished_tx, mut finished_rx) = mpsc::unbounded_channel();
    let release = Arc::new(Notify::new());
    let service_token = CancellationToken::new();
    let peer_info = InitializeRequestParams::new(
        ClientCapabilities::default(),
        Implementation::new("large-future-test", "0.0.0"),
    )
    .with_protocol_version(ProtocolVersion::V_2025_11_25);
    // Skip initialization to isolate the normal request-dispatch path.
    let server = serve_directly_with_ct::<RoleServer, _, _, _, _>(
        LargeResourceServer {
            started: started_tx,
            finished: finished_tx,
            release: release.clone(),
        },
        server_transport,
        Some(peer_info),
        service_token.clone(),
    );
    let mut client = IntoTransport::<RoleClient, _, _>::into_transport(client_transport);

    client
        .send(read_request(10, "test://cancel", "2025-06-18"))
        .await
        .unwrap();
    let (cancelled_id, cancelled_token) = tokio::time::timeout(WAIT, started_rx.recv())
        .await
        .expect("first request did not start")
        .unwrap();
    assert_eq!(cancelled_id, RequestId::Number(10));

    client
        .send(read_request(20, "test://complete", "2025-11-25"))
        .await
        .unwrap();
    let (completed_id, completed_token) = tokio::time::timeout(WAIT, started_rx.recv())
        .await
        .expect("second request did not start")
        .unwrap();
    assert_eq!(completed_id, RequestId::Number(20));

    let cancel: ClientJsonRpcMessage = serde_json::from_value(serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/cancelled",
        "params": { "requestId": 10, "reason": "large-future regression test" }
    }))
    .unwrap();
    client.send(cancel).await.unwrap();
    assert_eq!(
        tokio::time::timeout(WAIT, finished_rx.recv())
            .await
            .expect("first request did not observe cancellation")
            .unwrap(),
        (RequestId::Number(10), true),
    );
    assert!(cancelled_token.is_cancelled());
    assert!(!completed_token.is_cancelled());
    assert!(!service_token.is_cancelled());

    release.notify_one();
    assert_eq!(
        tokio::time::timeout(WAIT, finished_rx.recv())
            .await
            .expect("second request did not finish")
            .unwrap(),
        (RequestId::Number(20), false),
    );
    let message = tokio::time::timeout(WAIT, client.receive())
        .await
        .expect("successful response did not arrive")
        .expect("transport closed before the successful response");
    let ServerJsonRpcMessage::Response(response) = message else {
        panic!("expected successful response, got {message:?}");
    };
    // A cancelled handler's result must not leak onto the response stream.
    assert_eq!(response.id, RequestId::Number(20));
    assert!(matches!(response.result, ServerResult::ReadResourceResult(_)));
    // Normal response completion also retires/cancels its request token.
    assert!(completed_token.is_cancelled());
    assert!(!service_token.is_cancelled());
    assert!(ORIGINATING_REQUEST.try_with(|_| ()).is_err());
    assert!(ORIGINATING_PROTOCOL_VERSION.try_with(|_| ()).is_err());

    tokio::time::timeout(WAIT, server.cancel())
        .await
        .expect("server did not shut down")
        .unwrap();
}
