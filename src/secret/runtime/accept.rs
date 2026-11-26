use super::*;
use serde_json::json;

pub(super) fn snapshot(chat: ChatSnapshot) -> Value {
    json!({"chat_id":super::arguments::lifted(chat.id),"secret_chat_id":chat.id,"peer_user_id":chat.peer_user_id,"is_outbound":chat.is_outbound,"state":chat.state,"ttl":chat.ttl,"layer":chat.capability_layer,"key_fingerprint":chat.key_fingerprint,"has_previous_key":chat.has_previous_key,"exchange_in_progress":chat.exchange_in_progress,"paused":chat.paused})
}

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;

type Attempt = Arc<Mutex<Option<&'static str>>>;

#[derive(Default)]
pub(super) struct Acceptance {
    retired: AtomicBool,
    // A cancelled or ambiguous RPC remains attempted until the manager confirms it.
    attempts: Mutex<HashMap<i32, Attempt>>,
}

fn refused(code: &'static str) -> Value {
    json!({"status":"refused","code":code})
}
fn uncertain() -> Value {
    json!({"status":"unconfirmed","code":"awaiting_confirmation"})
}

impl Acceptance {
    pub(super) fn retire(&self) {
        self.retired.store(true, Ordering::Release);
    }

    async fn run<F, Fut>(
        &self,
        account: &str,
        id: i32,
        expected: Option<i64>,
        view: F,
        accept: impl FnOnce() -> Fut,
    ) -> Value
    where
        F: Fn() -> Result<ChatSnapshot, SecretError>,
        Fut: Future<Output = Result<ChatSnapshot, SecretError>>,
    {
        let inspect = |already, peer: Option<i64>| -> Result<i64, Value> {
            if self.retired.load(Ordering::Acquire) {
                return Err(refused("backend_unavailable"));
            }
            let chat = view().map_err(|e| match e {
                SecretError::UnknownChat { .. } => refused("not_found"),
                SecretError::ChatClosed { .. } => refused("closed"),
                _ => refused("backend_unavailable"),
            })?;
            if chat.id != id
                || chat.peer_user_id <= 0
                || [expected, peer]
                    .into_iter()
                    .flatten()
                    .any(|p| p != chat.peer_user_id)
                || chat.is_outbound
            {
                return Err(refused("not_pending_incoming"));
            }
            match chat.state {
                ChatState::Closed => Err(refused("closed")),
                ChatState::Ready => Err(
                    json!({"status":"ready","account":account,"chat_id":id.to_string(),"peer_user_id":chat.peer_user_id.to_string(),"already_ready":already}),
                ),
                ChatState::Waiting if !chat.paused => Ok(chat.peer_user_id),
                _ => Err(refused("not_pending_incoming")),
            }
        };
        let peer = match inspect(true, None) {
            Ok(p) => p,
            Err(body) => return body,
        };
        let attempt = self.attempts.lock().await.entry(id).or_default().clone();
        let mut previous = attempt.lock().await;
        if let Err(body) = inspect(true, Some(peer)) {
            return body;
        }
        match *previous {
            Some("awaiting_confirmation") => return uncertain(),
            Some(code) => return refused(code),
            None => (),
        }
        *previous = Some("awaiting_confirmation");
        let outcome = accept().await;
        if let Err(body) = inspect(false, Some(peer)) {
            if body["status"] == "ready" || body["code"] == "closed" {
                self.attempts.lock().await.remove(&id);
            }
            return body;
        }
        let code = match outcome {
            Err(
                SecretError::DhParameters
                | SecretError::DhFailure(_)
                | SecretError::PeerValue
                | SecretError::Fingerprint,
            ) => "handshake_rejected",
            Err(SecretError::UnknownChat { .. }) => "not_found",
            Err(SecretError::ChatClosed { .. }) => "closed",
            Err(
                SecretError::Authorization
                | SecretError::ManagerStopping
                | SecretError::IngressUnavailable
                | SecretError::NotRunning,
            ) => "backend_unavailable",
            _ => return uncertain(),
        };
        *previous = Some(code);
        refused(code)
    }
}

impl SecretRuntime {
    pub(super) async fn accept_incoming(&self, id: i32, peer: Option<i64>) -> Value {
        self.acceptance
            .run(
                &self.label,
                id,
                peer,
                || {
                    if self
                        .history
                        .is_forgotten(id)
                        .map_err(|_| SecretError::Storage)?
                    {
                        return Err(SecretError::UnknownChat { chat_id: id });
                    }
                    self.manager.status(id)
                },
                || self.manager.accept(id),
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    fn chat(state: ChatState, outbound: bool) -> ChatSnapshot {
        ChatSnapshot {
            id: 12,
            peer_user_id: 17,
            state,
            is_outbound: outbound,
            ttl: 0,
            capability_layer: 0,
            key_fingerprint: Some(987654),
            key_hash: Some(vec![1, 2]),
            has_previous_key: false,
            exchange_in_progress: false,
            paused: false,
        }
    }
    #[tokio::test]
    async fn event_and_manual_race_one_handshake_then_reconcile_without_retry() {
        let acceptance = Acceptance::default();
        let attempts = AtomicUsize::new(0);
        let confirmed = AtomicBool::new(false);
        let operation = |peer| {
            acceptance.run(
                "work",
                12,
                peer,
                || {
                    Ok(chat(
                        if confirmed.load(Ordering::SeqCst) {
                            ChatState::Ready
                        } else {
                            ChatState::Waiting
                        },
                        false,
                    ))
                },
                || async {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    tokio::task::yield_now().await;
                    Err(SecretError::Transport)
                },
            )
        };
        let (auto, manual) = tokio::join!(operation(Some(17)), operation(None));
        assert_eq!(auto, uncertain());
        assert_eq!(manual, uncertain());
        confirmed.store(true, Ordering::SeqCst);
        assert_eq!(
            operation(None).await,
            json!({"status":"ready","chat_id":"12","peer_user_id":"17","account":"work","already_ready":true})
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        acceptance.retire();
        assert_eq!(operation(Some(17)).await, refused("backend_unavailable"));
    }
    #[tokio::test]
    async fn accept_checks_direction_identity_and_confirmation_before_reporting_success() {
        let attempts = AtomicUsize::new(0);
        for (view, peer, body) in [
            (
                Ok(chat(ChatState::Waiting, true)),
                None,
                refused("not_pending_incoming"),
            ),
            (
                Ok(chat(ChatState::Waiting, false)),
                Some(88),
                refused("not_pending_incoming"),
            ),
            (Ok(chat(ChatState::Closed, false)), None, refused("closed")),
            (
                Err(SecretError::UnknownChat { chat_id: 12 }),
                None,
                refused("not_found"),
            ),
        ] {
            let result = Acceptance::default()
                .run(
                    "other",
                    12,
                    peer,
                    || view.clone(),
                    || async {
                        attempts.fetch_add(1, Ordering::SeqCst);
                        Err(SecretError::Transport)
                    },
                )
                .await;
            assert_eq!(result, body);
        }
        assert_eq!(attempts.load(Ordering::SeqCst), 0);
        for peer in [Some(17), None] {
            let acceptance = Acceptance::default();
            let ready = AtomicBool::new(false);
            let result = acceptance
                .run(
                    "work",
                    12,
                    peer,
                    || {
                        Ok(chat(
                            if ready.load(Ordering::SeqCst) {
                                ChatState::Ready
                            } else {
                                ChatState::Waiting
                            },
                            false,
                        ))
                    },
                    || async {
                        ready.store(true, Ordering::SeqCst);
                        Ok(chat(ChatState::Ready, false))
                    },
                )
                .await;
            assert_eq!(
                result,
                json!({"status":"ready","chat_id":"12","peer_user_id":"17","account":"work","already_ready":false})
            );
        }
        let rejected = Acceptance::default()
            .run(
                "work",
                12,
                None,
                || Ok(chat(ChatState::Waiting, false)),
                || async { Err(SecretError::Fingerprint) },
            )
            .await;
        assert_eq!(rejected, refused("handshake_rejected"));
    }
    #[tokio::test]
    async fn both_entries_reuse_the_connected_fixture_socket_for_bidirectional_messages() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::{TcpListener, TcpStream},
            time::{Duration, timeout},
        };
        timeout(Duration::from_secs(5), async {
            for peer in [Some(17), None] {
                let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                    .await
                    .unwrap();
                let connections = AtomicUsize::new(0);
                let handshakes = AtomicUsize::new(0);
                let (stream, remote) = tokio::join!(
                    TcpStream::connect(listener.local_addr().unwrap()),
                    listener.accept()
                );
                let mut stream = stream.unwrap();
                let mut remote = remote.unwrap().0;
                connections.fetch_add(1, Ordering::SeqCst);
                let ready = AtomicBool::new(false);
                let acceptance = Acceptance::default();
                let remote = async {
                    assert_eq!(remote.read_u8().await.unwrap(), 1);
                    remote.write_u8(2).await.unwrap();
                    assert_eq!(remote.read_u8().await.unwrap(), 3);
                    remote.write_u8(4).await.unwrap();
                };
                let local = async {
                    let view = || {
                        Ok(chat(
                            if ready.load(Ordering::SeqCst) {
                                ChatState::Ready
                            } else {
                                ChatState::Waiting
                            },
                            false,
                        ))
                    };
                    let result = acceptance
                        .run("work", 12, peer, view, || async {
                            handshakes.fetch_add(1, Ordering::SeqCst);
                            stream
                                .write_u8(1)
                                .await
                                .map_err(|_| SecretError::Transport)?;
                            if stream.read_u8().await.map_err(|_| SecretError::Transport)? != 2 {
                                return Err(SecretError::Fingerprint);
                            }
                            ready.store(true, Ordering::SeqCst);
                            view()
                        })
                        .await;
                    assert_eq!(result["already_ready"], false);
                    stream.write_u8(3).await.unwrap();
                    assert_eq!(stream.read_u8().await.unwrap(), 4);
                    let repeat = acceptance
                        .run("work", 12, None, view, || async {
                            panic!("duplicate handshake")
                        })
                        .await;
                    assert_eq!(repeat["already_ready"], true);
                };
                tokio::join!(local, remote);
                // Drain the listener backlog without sleeping: every second connection is counted.
                let listen = listener.into_std().unwrap();
                while listen.accept().is_ok() {
                    connections.fetch_add(1, Ordering::SeqCst);
                }
                assert_eq!(connections.load(Ordering::SeqCst), 1);
                assert_eq!(handshakes.load(Ordering::SeqCst), 1);
            }
        })
        .await
        .unwrap();
    }
}
