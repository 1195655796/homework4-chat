use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;

use anyhow::Result;
use dashmap::DashMap;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{channel, Sender};
use tracing::{error, info, warn};
use tracing_subscriber;

#[derive(Debug)]
struct Message {
    from: String,
    content: String,
}

struct ChatState {
    peers: DashMap<SocketAddr, (String, Sender<Arc<Message>>)>,
}

impl ChatState {
    async fn broadcast_message(&self, message: Arc<Message>, exclude_addr: Option<SocketAddr>) {
        for peer in self.peers.iter() {
            if Some(*peer.key()) == exclude_addr {
                continue;
            }
            let (_, tx) = peer.value();
            if let Err(e) = tx.send(message.clone()).await {
                warn!("Failed to send message to {}: {}", peer.key(), e);
            }
        }
    }

    async fn remove_peer(&self, addr: &SocketAddr) {
        self.peers.remove(addr);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let listener = TcpListener::bind("0.0.0.0:8080").await?;
    info!("Listening on 0.0.0.0:8080");

    let state = Arc::new(ChatState {
        peers: DashMap::new(),
    });

    loop {
        let (stream, addr) = listener.accept().await?;
        info!("New client connected: {}", addr);

        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_client(state, stream, addr).await {
                error!("Error handling client {}: {}", addr, e);
            }
        });
    }
}

async fn handle_client(state: Arc<ChatState>, stream: TcpStream, addr: SocketAddr) -> Result<()> {
    let (reader, writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let writer = Arc::new(Mutex::new(writer));
    let (tx, mut rx) = channel::<Arc<Message>>(100);

    // Prompt user for their username
    writer.lock().await.write_all(b"Enter username: ").await?;

    // Read the username from the client
    let mut buf = String::new();
    reader.read_line(&mut buf).await?;
    let username = buf.trim().to_string();
    buf.clear();
    // Welcome message
    writer.lock().await.write_all(format!("Welcome, {}!\n", username).as_bytes()).await?;

    state.peers.insert(addr, (username.clone(), tx));

    let join_message = Arc::new(Message {
        from: "System".to_string(),
        content: format!("[{} has joined the chat]", username),
    });

    // Log and broadcast join message
    info!("{}", join_message.content);
    state.broadcast_message(join_message.clone(), None).await;

    let writer_clone = writer.clone();
    tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            let display_message = if message.from == "System" {
                format!("System: {}\n", message.content)
            } else {
                format!("{}: {}\n", message.from, message.content)
            };

            if let Err(e) = writer_clone.lock().await.write_all(display_message.as_bytes()).await {
                error!("Error sending message to {}: {}", addr, e);
                break;
            }
        }
    });

    while reader.read_line(&mut buf).await? != 0 {
        let message_content = buf.trim().to_string();

        // Broadcast the message to all peers, excluding the sender
        let message = Arc::new(Message {
            from: username.clone(),
            content: message_content.clone(),
        });
        state.broadcast_message(message.clone(), Some(addr)).await;

        // Clear the buffer for the next message
        buf.clear();
    }

    let leave_message = Arc::new(Message {
        from: "System".to_string(),
        content: format!("[{} has left the chat :(]", username),
    });

    // Log and broadcast leave message
    info!("{}", leave_message.content);
    state.broadcast_message(leave_message, None).await;
    state.remove_peer(&addr).await;

    Ok(())
}
