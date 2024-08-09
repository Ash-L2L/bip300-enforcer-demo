use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    str::FromStr,
};

use clap::{Parser, ValueEnum};
use serde::Deserialize;

const DEFAULT_SOCKET_ADDR: SocketAddr =
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 8332));

#[derive(Clone, ValueEnum)]
pub enum Network {
    Mainnet,
    Testnet,
    Regtest,
}

impl From<Network> for bitcoin::Network {
    fn from(network: Network) -> Self {
        match network {
            Network::Mainnet => bitcoin::Network::Bitcoin,
            Network::Testnet => bitcoin::Network::Testnet,
            Network::Regtest => bitcoin::Network::Regtest,
        }
    }
}

#[derive(Clone, Debug, Parser)]
pub struct RpcAuth {
    /// Bitcoin node RPC pass
    #[arg(long, default_value = "")]
    pub rpc_pass: String,
    /// Bitcoin node RPC user
    #[arg(long, default_value = "")]
    pub rpc_user: String,
}

/// Specification for how many invalid txs will be in a block, and the reason
/// that they are invalid
#[derive(Clone, Debug, Deserialize)]
pub struct BlockSpec {
    /// Coinbase output contains duplicate M2 messages
    #[serde(default)]
    pub duplicate_m2: bool,
}

impl BlockSpec {
    /// `true` IFF an M1 message is required in a previous block
    pub fn requires_m1(&self) -> bool {
        self.duplicate_m2
    }

    /// Calculate the number of reasons for which the specified block will be
    /// invalid
    pub fn n_reasons_invalid(&self) -> usize {
        let mut res = 0;
        let Self { duplicate_m2 } = self;
        if *duplicate_m2 {
            res += 1;
        }
        res
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(transparent)]
pub struct BlocksSpec(pub Vec<BlockSpec>);

impl BlocksSpec {
    /// `true` IFF an M1 message is required in a previous block
    pub fn requires_m1(&self) -> bool {
        self.0.iter().any(|block_spec| block_spec.requires_m1())
    }
}

impl FromStr for BlocksSpec {
    type Err = serde_path_to_error::Error<serde_json::Error>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut deserializer = serde_json::Deserializer::from_str(s);
        let res = serde_path_to_error::deserialize(&mut deserializer)?;
        Ok(Self(res))
    }
}

#[derive(Parser)]
pub struct Cli {
    /// Blocks spec as a JSON string
    pub blocks_spec: BlocksSpec,
    #[arg(global(true), long, value_enum, default_value_t = Network::Regtest)]
    pub network: Network,
    /// Socket address for the node RPC server
    #[arg(long, default_value_t = DEFAULT_SOCKET_ADDR)]
    pub rpc_addr: SocketAddr,
    #[command(flatten)]
    pub rpc_auth: RpcAuth,
}
