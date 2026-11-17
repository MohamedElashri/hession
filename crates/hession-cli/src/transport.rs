//! SDK transport with bounded in-flight request bookkeeping and pipe backpressure.
use rmcp::{
    model::{ClientNotification, JsonRpcMessage, RequestId},
    service::{RxJsonRpcMessage, TxJsonRpcMessage},
    transport::Transport,
    RoleServer,
};
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
const MAX_IN_FLIGHT: usize = 16;
struct PendingRequest {
    _permit: OwnedSemaphorePermit,
    sending: bool,
}
pub(crate) struct BoundedTransport<T> {
    inner: T,
    slots: Arc<Semaphore>,
    pending: Arc<Mutex<HashMap<RequestId, PendingRequest>>>,
}
impl<T> BoundedTransport<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            slots: Arc::new(Semaphore::new(MAX_IN_FLIGHT)),
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}
impl<T: Transport<RoleServer> + 'static> Transport<RoleServer> for BoundedTransport<T> {
    type Error = T::Error;
    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let id = match &item {
            JsonRpcMessage::Response(r) => Some(r.id.clone()),
            JsonRpcMessage::Error(e) => e.id.clone(),
            _ => None,
        };
        // The SDK detaches response writes. Cancellation must never release a
        // queued response's slot while a non-reading client blocks its pipe.
        if let Some(id) = &id {
            if let Some(request) = self
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_mut(id)
            {
                request.sending = true;
            }
        }
        let future = self.inner.send(item);
        let pending = self.pending.clone();
        async move {
            let result = future.await;
            if let Some(id) = id {
                pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
            }
            result
        }
    }
    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        let permit = self.slots.clone().acquire_owned().await.ok()?;
        let message = self.inner.receive().await?;
        if let JsonRpcMessage::Request(request) = &message {
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            // Duplicate IDs cannot release/reuse another request's permit.
            // Close malformed streams instead of accumulating duplicate tasks.
            if pending.contains_key(&request.id) {
                return None;
            }
            pending.insert(
                request.id.clone(),
                PendingRequest {
                    _permit: permit,
                    sending: false,
                },
            );
        } else {
            drop(permit);
            if let JsonRpcMessage::Notification(notification) = &message {
                if let ClientNotification::CancelledNotification(cancel) =
                    &notification.notification
                {
                    if let Some(id) = &cancel.params.request_id {
                        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
                        if pending.get(id).is_some_and(|request| !request.sending) {
                            pending.remove(id);
                        }
                    }
                }
            }
        }
        Some(message)
    }
    async fn close(&mut self) -> Result<(), Self::Error> {
        self.slots.close();
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.inner.close().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::transport::async_rw::AsyncRwTransport;
    use serde_json::json;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    #[test]
    fn admission_is_bounded_and_response_backpressure_holds_its_slot() {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let (mut writer, input)=tokio::io::duplex(64*1024);
            let (output, reader)=tokio::io::duplex(64);
            let mut transport=BoundedTransport::new(AsyncRwTransport::<RoleServer,_,_>::new(input,output));
            for id in 0..17 {writer.write_all(format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"ping\"}}\n").as_bytes()).await.unwrap();}
            for _ in 0..16 {assert!(transport.receive().await.is_some());}
            assert_eq!(transport.pending.lock().unwrap().len(),16);
            assert!(tokio::time::timeout(Duration::from_millis(20),transport.receive()).await.is_err());
            let response=serde_json::from_value(json!({"jsonrpc":"2.0","id":0,"result":{"content":[{"type":"text","text":"x".repeat(4096)}]}})).unwrap();
            let send=transport.send(response);tokio::pin!(send);
            assert!(tokio::time::timeout(Duration::from_millis(20),&mut send).await.is_err());
            assert_eq!(transport.pending.lock().unwrap().len(),16);
            assert!(tokio::time::timeout(Duration::from_millis(20),transport.receive()).await.is_err());
            let drain=async {let mut line=String::new();BufReader::new(reader).read_line(&mut line).await.unwrap();serde_json::from_str::<serde_json::Value>(&line).unwrap()};
            let (sent,response)=tokio::join!(&mut send,drain);sent.unwrap();assert_eq!(response["id"],0);
            assert!(transport.receive().await.is_some());assert_eq!(transport.pending.lock().unwrap().len(),16);
            transport.close().await.unwrap();assert!(transport.pending.lock().unwrap().is_empty());
        });
    }
    #[test]
    fn cancelling_a_queued_response_does_not_release_its_backpressure_slot() {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async{
            let (mut writer,input)=tokio::io::duplex(4096);let (output,_reader)=tokio::io::duplex(64);
            let mut transport=BoundedTransport::new(AsyncRwTransport::<RoleServer,_,_>::new(input,output));
            writer.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n").await.unwrap();assert!(transport.receive().await.is_some());
            {
                let response=serde_json::from_value(json!({"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"x".repeat(4096)}]}})).unwrap();
                let send=transport.send(response);tokio::pin!(send);assert!(tokio::time::timeout(Duration::from_millis(20),&mut send).await.is_err());
                writer.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":1}}\n").await.unwrap();assert!(transport.receive().await.is_some());
                assert_eq!(transport.pending.lock().unwrap().len(),1);
                for id in 2..17 {writer.write_all(format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"ping\"}}\n").as_bytes()).await.unwrap();assert!(transport.receive().await.is_some());}
                assert_eq!(transport.pending.lock().unwrap().len(),16);
                assert!(tokio::time::timeout(Duration::from_millis(20),transport.receive()).await.is_err());
            }
            transport.close().await.unwrap();
        });
    }
    #[test]
    fn duplicate_pending_ids_close_the_stream_without_replacing_the_slot() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let (mut writer, input) = tokio::io::duplex(1024);
                let (output, _) = tokio::io::duplex(1024);
                let mut transport =
                    BoundedTransport::new(AsyncRwTransport::<RoleServer, _, _>::new(input, output));
                for _ in 0..2 {
                    writer
                        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n")
                        .await
                        .unwrap();
                }
                assert!(transport.receive().await.is_some());
                assert!(transport.receive().await.is_none());
                assert_eq!(transport.pending.lock().unwrap().len(), 1);
                transport.close().await.unwrap();
            });
    }
}
