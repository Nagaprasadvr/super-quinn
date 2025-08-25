# super-quinn

**super-quinn** is a wrapper around the [quinn](https://github.com/quinn-rs/quinn) QUIC library that makes it easy to set up secure QUIC networking in Rust.  
It provides higher-level abstractions for **TLS certificate management**, **message serialization/deserialization**, and a **message handler system** for structured communication.

---

## ✨ Features

- 🚀 Simplified **QUIC server & client** setup
- 🔐 Built-in **TLS** with `.pem` certificate handling
- 📦 **Serde-based serialization & deserialization**
- 🛠️ **Message handler registry** to route messages by type/discriminator
- 📂 Load multiple allowed server certs from a directory

---

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
anyhow = "1.0.99"
borsh = { version = "1.5.7", features = ["derive"] }
num-derive = "0.4.2"
num-traits = "0.2.19"
super-quinn = { path = "../super-quinn" }
tokio = { version = "1.47.1", features = ["rt-multi-thread", "tokio-macros"] }
tracing = "0.1.41"
tracing-subscriber = "0.3.19"
```

## Setup

- Add a config file [`config.toml`](./config.toml)
- Generate RSA private keys and CA for TLS layer to exchange keys securely, authenticate server and to have an encrypted communication
  check [gen-keys.sh](./gen-keys.sh)
- Sending messages to a server requires CA certificates for `Authentication` so copy the server CA certificates to `keys/client/tls-allowed-servers-certs.pem`. If Client A is sending a message to Server B then client A needs the CA cert of server B, so just copy the cert and paste it in the above `.pem` path

## Usage

- #### Define your Message structure

```rust
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct SolanaMessage {
    pub id: u8,
    pub message: String,
}

#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct EthereumMessage {
    pub id: u8,
    pub message: String,
}

#[repr(u16)]
#[derive(Debug, FromPrimitive)]
pub enum MyMessageType {
    Solana,
    Ethereum,
}

impl Message for SolanaMessage {
    const MSG_TYPE_DISCRIMINATOR: u16 = 0;

    fn message_len(&self) -> u16 {
        (size_of::<usize>() + 1 + self.message.len()) as u16
    }

    fn pack(&self) -> SuperQuinnResult<Vec<u8>> {
        borsh::to_vec(self).map_err(|_e| SuperQuinnError::InvalidMessage)
    }

    fn unpack(bytes: &[u8]) -> SuperQuinnResult<Self> {
        borsh::from_slice(bytes).map_err(|_e| SuperQuinnError::InvalidMessage)
    }
}

impl Message for EthereumMessage {
    const MSG_TYPE_DISCRIMINATOR: u16 = 0;

    fn message_len(&self) -> u16 {
        (size_of::<usize>() + 1 + self.message.len()) as u16
    }

    fn pack(&self) -> SuperQuinnResult<Vec<u8>> {
        borsh::to_vec(self).map_err(|_e| SuperQuinnError::InvalidMessage)
    }

    fn unpack(bytes: &[u8]) -> SuperQuinnResult<Self> {
        borsh::from_slice(bytes).map_err(|_e| SuperQuinnError::InvalidMessage)
    }
}
```

- #### Define MessageHandler which handles incoming raw messages received by server

```rust
#[derive(Clone)]
pub struct MyMessageHandler {}

impl MessageHandler for MyMessageHandler {
    async fn handle_raw_msg(
        &self,
        raw_msg: RawMessage,
        addr: SocketAddr,
    ) -> super_quinn::error::SuperQuinnResult<()> {
        println!("Address : {addr}");
        let msg_type = MyMessageType::from_u16(raw_msg.msg_type());

        match msg_type {
            Some(m) => match m {
                MyMessageType::Ethereum => {}
                MyMessageType::Solana => {
                    let data = SolanaMessage::unpack(raw_msg.msg_data());
                    println!("data : {:?}", data);
                }
            },
            None => {
                println!("None")
            }
        }
        Ok(())
    }
}
```

- #### Run the QuicServer on tokio main

```rust
#[tokio::main(flavor = "multi_thread")]
async fn main() -> SuperQuinnResult<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    let config = SuperQuinnConfig::parse();

    let (server, rx) = QuicServer::spawn(&config);

    let msg_processor = MessageProcessorBuilder::new()
        .with_receiver(rx)
        .with_handler(MyMessageHandler {})
        .build()
        .start();

    msg_processor.await??;
    server.await??;

    Ok(())
}
```

- #### Run the QuicClient on tokio main

```rust
#[tokio::main(flavor = "multi_thread")]
async fn main() -> SuperQuinnResult<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    let config = SuperQuinnConfig::parse();

    let (client, tx) = QuicClient::spawn(&config);

    let msg_sender = MessageSender::new(tx);

    let msgs = (0..10)
        .into_iter()
        .filter_map(|_| {
            if let Ok(p) = (SolanaMessage {
                id: 0,
                message: "hello solana".to_string(),
            })
            .pack()
            {
                Some(RawMessage::new(0, p))
            } else {
                None
            }
        })
        .collect();

    msg_sender
        .send_msgs(vec![RawMsgsWithAddr(
            msgs,
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 4443)),
        )])
        .await
        .map_err(|e| SuperQuinnError::SendMsgError(e))?;

    client.await??;

    Ok(())
}
```

- **Both server and client are independent and will run on separate tokio threads, so using this we can also build a node which acts both as server and client**
