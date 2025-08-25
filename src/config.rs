use serde::Deserialize;
use std::{fs, net::SocketAddr, str::FromStr, sync::Arc};

#[derive(Debug, Deserialize)]
struct ServerConfigToml {
    port: u16,
    host: String,
    tls_cert_path: String,
    tls_pvt_key_path: String,
}

#[derive(Debug, Deserialize)]
struct ClientConfigToml {
    port: u16,
    host: String,
    tls_allowed_servers_certs_path: String,
}

#[derive(Debug, Deserialize)]
pub struct SuperQuinnConfig {
    server: ServerConfigToml,
    client: ClientConfigToml,
}

impl SuperQuinnConfig {
    pub fn parse() -> Arc<Self> {
        let content = fs::read_to_string("config.toml").expect("Could not read file");
        Arc::new(toml::from_str(&content).expect("Invalid TOML format"))
    }

    pub fn server_port(&self) -> u16 {
        self.server.port
    }

    pub fn server_addr(&self) -> String {
        self.server.host.to_string()
    }

    pub fn server_addr_with_port(&self) -> String {
        format!("{}:{}", self.server_addr(), self.server_port())
    }

    pub fn server_socket_addr(&self) -> SocketAddr {
        SocketAddr::from_str(&self.server_addr_with_port()).expect("invalid socket address")
    }

    pub fn client_port(&self) -> u16 {
        self.client.port
    }

    pub fn client_addr(&self) -> String {
        self.client.host.to_string()
    }

    pub fn client_socket_addr(&self) -> SocketAddr {
        SocketAddr::from_str(&self.client_addr_with_port()).expect("invalid socket address")
    }

    pub fn client_addr_with_port(&self) -> String {
        format!("{}:{}", self.client_addr(), self.client_port())
    }

    pub fn client_allowed_server_certs_path(&self) -> &String {
        &self.client.tls_allowed_servers_certs_path
    }

    pub fn server_rsa_cert_path(&self) -> &String {
        &self.server.tls_cert_path
    }

    pub fn server_rsa_key_path(&self) -> &String {
        &self.server.tls_pvt_key_path
    }
}
