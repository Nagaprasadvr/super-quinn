use std::{
    fs::{self},
    net::SocketAddr,
    path::Path,
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, bail};
use quinn::{
    Endpoint, VarInt,
    crypto::rustls::{QuicClientConfig, QuicServerConfig},
    rustls::{
        self, RootCertStore,
        pki_types::{CertificateDer, PrivateKeyDer},
    },
};
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::task::{JoinHandle, JoinSet};
use tracing::{debug, error, info, warn};

use crate::{
    config::SuperQuinnConfig,
    error::SuperQuinnResult,
    message::{RawMsgsWithAddr, WireMessageWithAddr},
};

pub const ALPN_QUIC_HTTP: &[&[u8]] = &[b"hq-29"];

pub const CLOSE_CONN_MESSAGE: &[u8] = b"CLOSE_CONN";

pub const CLOSE_CONN_CODE: VarInt = VarInt::from_u32(0);

pub const MAX_QUIC_MESSAGE_BYTES: u64 = 1_024; // 1kB

pub const MAX_CONCURRENCY: usize = 10;

pub const MAX_CHAN_SIZE: usize = 1024;

use rustls::pki_types::PrivatePkcs8KeyDer;

fn load_certs_and_key(
    cert_path: &Path,
    key_path: &Path,
) -> anyhow::Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    let cert_chain = load_certs(cert_path)?;

    // Read private key file
    let key_bytes = fs::read(key_path)?;

    // Parse private key (supports PEM or DER)
    let pkcs8_keys: Vec<_> = rustls_pemfile::pkcs8_private_keys(&mut &*key_bytes)
        .into_iter()
        .filter_map(|k| k.ok())
        .collect();
    if pkcs8_keys.is_empty() {
        bail!("no private keys found in PEM file");
    }
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pkcs8_keys[0].clone_key()));

    Ok((cert_chain, key))
}

fn load_certs(cert_path: &Path) -> anyhow::Result<Vec<CertificateDer<'static>>> {
    // Read certificate file
    let cert_bytes = fs::read(cert_path)?;

    // Parse certificate chain (supports PEM or DER)
    let cert_chain: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut &*cert_bytes)
        .into_iter()
        .filter_map(|c| c.ok())
        .map(CertificateDer::from)
        .collect();

    if cert_chain.is_empty() {
        bail!("no valid certificates found in {}", cert_path.display());
    }

    Ok(cert_chain)
}

pub struct QuicServer;

impl QuicServer {
    pub fn spawn(
        config: &Arc<SuperQuinnConfig>,
    ) -> (JoinHandle<SuperQuinnResult<()>>, Receiver<RawMsgsWithAddr>) {
        let config = Arc::clone(&config);
        let (tx, rx) = mpsc::channel::<RawMsgsWithAddr>(MAX_CHAN_SIZE);
        debug!("quic::server::spawn : quicServerProcessorThread");
        let handle = tokio::spawn(Self::spawn_inner(config, tx));

        (handle, rx)
    }

    pub async fn spawn_inner(
        config: Arc<SuperQuinnConfig>,
        tx: Sender<RawMsgsWithAddr>,
    ) -> SuperQuinnResult<()> {
        let (certs, key) = load_certs_and_key(
            Path::new(&config.server_rsa_cert_path()),
            Path::new(&config.server_rsa_key_path()),
        )?;

        let mut server_crypto = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .context("server config failed")?;

        server_crypto.alpn_protocols = ALPN_QUIC_HTTP.iter().map(|&x| x.into()).collect();

        server_crypto.key_log = Arc::new(rustls::KeyLogFile::new());

        let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(
            QuicServerConfig::try_from(server_crypto).context("server config failed")?,
        ));

        Arc::get_mut(&mut server_config.transport)
            .context("failed to get server config mut ref")?
            .max_concurrent_uni_streams(0_u8.into());

        // Build server endpoint at configured address
        let server = Endpoint::server(
            server_config,
            std::net::SocketAddr::from_str(&config.server_addr_with_port())
                .context("Invalid server address format")?,
        )
        .context("Failed to initialize QUIC server endpoint")?;

        info!(
            "quick::server running at {}",
            config.server_addr_with_port()
        );

        let mut tasks = JoinSet::new();
        while let Some(connecting) = server.accept().await {
            let tx = tx.clone();

            if tasks.len() >= MAX_CONCURRENCY {
                tasks.join_next().await;
            }

            tasks.spawn(async move {
                let connection = match connecting.await {
                    Ok(conn) => {
                        debug!("quic::server incoming conn from {}", conn.remote_address());
                        conn
                    }
                    Err(e) => {
                        error!("quic::server connection - {}", e);
                        return;
                    }
                };

                let remote_addr = connection.remote_address();
                loop {
                    let (mut _send, mut recv) = match connection.accept_bi().await {
                        Ok(recv) => recv,
                        Err(e) => {
                            connection.close(CLOSE_CONN_CODE, CLOSE_CONN_MESSAGE);
                            warn!("quic::server connection closed: {}", e);
                            break;
                        }
                    };

                    match recv.read_to_end(MAX_QUIC_MESSAGE_BYTES as usize).await {
                        Ok(data) => {
                            let wire_msg = WireMessageWithAddr(data, remote_addr);
                            if let Ok(raw_msgs) = wire_msg.to_raw_msgs() {
                                if let Err(e) = tx.send(raw_msgs).await {
                                    error!("quic::server channel send error: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            error!("quic::server error reading bytes: {}", e);
                        }
                    }
                }
            });
        }

        tasks.join_all().await;

        Ok(())
    }
}

pub struct QuicClient;

impl QuicClient {
    pub fn spawn(
        config: &Arc<SuperQuinnConfig>,
    ) -> (JoinHandle<anyhow::Result<()>>, Sender<Vec<RawMsgsWithAddr>>) {
        let config = Arc::clone(&config);
        let (tx, rx) = mpsc::channel::<Vec<RawMsgsWithAddr>>(MAX_CHAN_SIZE);
        debug!("quic::client::spawn : quicClientProcessorThread");
        let handle = tokio::spawn(Self::spawn_inner(config, rx));

        (handle, tx)
    }

    pub async fn spawn_inner(
        config: Arc<SuperQuinnConfig>,
        mut msg_receiver: Receiver<Vec<RawMsgsWithAddr>>,
    ) -> anyhow::Result<()> {
        let certs = load_certs(Path::new(&config.client_allowed_server_certs_path()))?;
        let mut root_cert_store = RootCertStore::empty();

        if certs.is_empty() {
            bail!("No certificates found in PEM file");
        }

        for cert_der in certs {
            root_cert_store.add(CertificateDer::from(cert_der))?;
        }

        let mut client_crypto = rustls::ClientConfig::builder()
            .with_root_certificates(root_cert_store)
            .with_no_client_auth();

        client_crypto.alpn_protocols = ALPN_QUIC_HTTP.iter().map(|&x| x.into()).collect();

        client_crypto.key_log = Arc::new(rustls::KeyLogFile::new());

        let quic_client_config =
            QuicClientConfig::try_from(client_crypto).context("Failed to convert rustls config")?;

        let mut endpoint = quinn::Endpoint::client(config.client_socket_addr())
            .context("Failed to create endpoint")?;

        endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(quic_client_config)));

        let endpoint = Arc::new(endpoint);

        while let Ok(msgs) = msg_receiver.try_recv() {
            let mut tasks = JoinSet::new();
            for m in msgs {
                let endpoint = Arc::clone(&endpoint);

                if tasks.len() >= MAX_CONCURRENCY {
                    tasks.join_next().await;
                }

                tasks.spawn(async move {
                    let wire_msg = WireMessageWithAddr::from_raw_msgs(m);
                    let receiver = wire_msg.1;
                    match Self::send_msg(&endpoint, wire_msg.data(), receiver).await {
                        Ok(_) => {}
                        Err(e) => error!("quic::send_msg err sending messages {:?}", e),
                    }
                });
            }

            tasks.join_all().await;
        }

        Ok(())
    }

    pub async fn send_msg(
        client: &Endpoint,
        data: Vec<u8>,
        receiver: SocketAddr,
    ) -> SuperQuinnResult<()> {
        // connect to server
        let connection = client.connect(receiver, "localhost").unwrap().await?;

        debug!("quic::connected -> {}", connection.remote_address());

        match connection.open_bi().await {
            Ok((mut send, _recv)) => match send.write(&data.as_slice()).await {
                Ok(len) => debug!("quic::send_msg wire_msg_len {}", len),
                Err(e) => error!("quic::send_msg error {}", e),
            },
            Err(e) => error!("quic::send_msg error {}", e),
        }

        tokio::time::sleep(Duration::from_secs(2)).await;

        Ok(())
    }
}
