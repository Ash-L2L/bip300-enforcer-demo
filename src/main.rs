use std::{
    net::SocketAddr,
    time::{Duration, SystemTime},
};

use bip300301::{client::BlockTemplate, MainClient as _};
use bitcoin::{
    absolute::LockTime,
    block::Header,
    constants::{COINBASE_MATURITY, SUBSIDY_HALVING_INTERVAL},
    hashes::{sha256d, Hash as _},
    opcodes::{all::OP_RETURN, OP_TRUE},
    transaction, Address, Amount, Block, BlockHash, CompactTarget, OutPoint,
    ScriptBuf, Sequence, Target, Transaction, TxIn, TxMerkleNode, TxOut,
    Witness,
};
use clap::Parser;

mod cli;
mod posix_script_builder;

use cli::{BlockSpec, BlocksSpec, Cli, RpcAuth};
use posix_script_builder::OutputPosixScriptBuilder;

/// Script with no spend requirements
fn unlocked_script() -> ScriptBuf {
    ScriptBuf::builder().push_opcode(OP_TRUE).into_script()
}

fn block_subsidy(network: bitcoin::Network, height: u32) -> Amount {
    #[allow(clippy::wildcard_in_or_patterns)]
    let halving_interval = match network {
        bitcoin::Network::Regtest => 150,
        bitcoin::Network::Bitcoin | bitcoin::Network::Testnet | _ => {
            SUBSIDY_HALVING_INTERVAL
        }
    };
    let epoch = height / halving_interval;
    Amount::from_int_btc(50) / (1 << epoch)
}

fn gen_block(
    prev_blockhash: BlockHash,
    target: CompactTarget,
    height: u32,
    coinbase_txouts: Vec<TxOut>,
    mut txs: Vec<Transaction>,
) -> anyhow::Result<Block> {
    let coinbase_txin = TxIn {
        previous_output: OutPoint::null(),
        script_sig: ScriptBuf::builder().push_int(height as i64).into_script(),
        // FIXME: Verify that this is correct
        sequence: Sequence::MAX,
        witness: Witness::new(),
    };
    let coinbase_tx = Transaction {
        version: transaction::Version::TWO,
        lock_time: LockTime::from_height(height + COINBASE_MATURITY)?,
        input: vec![coinbase_txin],
        output: coinbase_txouts,
    };
    txs.reverse();
    txs.push(coinbase_tx);
    txs.reverse();
    let header = Header {
        version: bitcoin::block::Version::NO_SOFT_FORK_SIGNALLING,
        prev_blockhash,
        merkle_root: TxMerkleNode::all_zeros(),
        time: SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs() as u32,
        bits: target,
        nonce: 0,
    };
    let mut block = Block {
        header,
        txdata: txs,
    };
    block.header.merkle_root = block.compute_merkle_root().unwrap();
    let target = Target::from_compact(target);
    let mut nonce = block.header.nonce;
    let mut header_bytes = bitcoin::consensus::serialize(&block.header);
    loop {
        let header_hash = sha256d::Hash::hash(&header_bytes).to_byte_array();
        if Target::from_le_bytes(header_hash) < target {
            break;
        }
        nonce += 1;
        let nonce_bytes = nonce.to_be_bytes();
        header_bytes[76] = nonce_bytes[0];
        header_bytes[77] = nonce_bytes[1];
        header_bytes[78] = nonce_bytes[2];
        header_bytes[79] = nonce_bytes[3];
    }
    block.header = bitcoin::consensus::deserialize(&header_bytes).unwrap();
    assert!(block.header.validate_pow(target).is_ok());
    Ok(block)
}

fn m1_txout(sidechain_number: u8, description: Vec<u8>) -> TxOut {
    let script_pubkey = ScriptBuf::from_bytes(
        std::iter::once(OP_RETURN.to_u8())
            .chain([0xD5, 0xE0, 0xC4, 0xAF, sidechain_number])
            .chain(description)
            .collect(),
    );
    TxOut {
        value: Amount::ZERO,
        script_pubkey,
    }
}

fn m2_txout(sidechain_number: u8, description: &[u8]) -> TxOut {
    let script_pubkey = ScriptBuf::from_bytes(
        std::iter::once(OP_RETURN.to_u8())
            .chain([0xD5, 0xE0, 0xC4, 0xAF, sidechain_number])
            .chain(sha256d::Hash::hash(description).to_byte_array())
            .collect(),
    );
    TxOut {
        value: Amount::ZERO,
        script_pubkey,
    }
}

const DEMO_SIDECHAIN_SLOT: u8 = 0xFF;
const DEMO_SIDECHAIN_DESCRIPTION: &[u8] = b"demo sidechain";

/// Generate initial setup blocks that ensure proposals exist, etc
async fn gen_setup_blocks(
    network: bitcoin::Network,
    rpc_addr: SocketAddr,
    rpc_auth: RpcAuth,
    blocks_spec: &BlocksSpec,
) -> anyhow::Result<Vec<Block>> {
    const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
    let mut blocks = Vec::new();
    let client = bip300301::client(
        rpc_addr,
        &rpc_auth.rpc_pass,
        Some(REQUEST_TIMEOUT),
        &rpc_auth.rpc_user,
    )?;
    let BlockTemplate {
        mut height,
        prev_blockhash,
        target,
        ..
    } = client.get_block_template(Default::default()).await?;
    let mut prev_blockhash =
        BlockHash::from_byte_array(*prev_blockhash.as_ref());
    let mut target = CompactTarget::from_consensus(target.to_consensus());
    let addr = Address::p2wsh(&unlocked_script(), network);
    let coinbase_value = block_subsidy(network, height);
    let coinbase_txout = TxOut {
        value: coinbase_value,
        script_pubkey: addr.script_pubkey(),
    };
    let block = gen_block(
        prev_blockhash,
        target,
        height,
        vec![coinbase_txout],
        Vec::new(),
    )?;
    prev_blockhash = block.block_hash();
    height = block.bip34_block_height()? as u32;
    target = block.header.target().to_compact_lossy();
    blocks.push(block);
    if blocks_spec.requires_m1() {
        let value_txout = TxOut {
            value: coinbase_value,
            script_pubkey: addr.script_pubkey(),
        };
        let m1_txout =
            m1_txout(DEMO_SIDECHAIN_SLOT, DEMO_SIDECHAIN_DESCRIPTION.to_vec());
        let coinbase_txouts = vec![value_txout, m1_txout];
        let block =
            gen_block(prev_blockhash, target, height, coinbase_txouts, vec![])?;
        blocks.push(block);
    }
    Ok(blocks)
}

/// Generate a comment for the block generated by a block spec
fn gen_comment(block_spec: &BlockSpec) -> String {
    let mut comment = vec![format!(
        "Generate a block with {} invalid conditions:",
        block_spec.n_reasons_invalid()
    )];
    let BlockSpec { duplicate_m2 } = block_spec;
    if *duplicate_m2 {
        comment.push("- 1 duplicate M2 message in coinbase outputs".to_owned());
    }
    comment.join("\n")
}

/// Generate coinbase txouts and txs from a block spec.
fn gen_txs(block_spec: &BlockSpec) -> (Vec<TxOut>, Vec<Transaction>) {
    let mut coinbase_txouts = Vec::new();
    let mut txs = Vec::new();
    let BlockSpec { duplicate_m2 } = block_spec;
    if *duplicate_m2 {
        let m2_txout =
            m2_txout(DEMO_SIDECHAIN_SLOT, DEMO_SIDECHAIN_DESCRIPTION);
        coinbase_txouts.push(m2_txout.clone());
        coinbase_txouts.push(m2_txout);
    }
    (coinbase_txouts, txs)
}

async fn gen_script(
    network: bitcoin::Network,
    rpc_addr: SocketAddr,
    rpc_auth: RpcAuth,
    blocks_spec: BlocksSpec,
) -> anyhow::Result<()> {
    let mut posix_script_builder =
        OutputPosixScriptBuilder::new(rpc_addr, rpc_auth.clone());
    let setup_blocks =
        gen_setup_blocks(network, rpc_addr, rpc_auth, &blocks_spec).await?;
    let mut height = setup_blocks.last().unwrap().bip34_block_height()? as u32;
    let mut prev_blockhash = setup_blocks.last().unwrap().block_hash();
    let mut target = setup_blocks
        .last()
        .unwrap()
        .header
        .target()
        .to_compact_lossy();
    posix_script_builder.comment("Mine some setup blocks");
    for block in setup_blocks {
        posix_script_builder.submitblock(&block);
    }
    for block_spec in blocks_spec.0.into_iter() {
        let comment = gen_comment(&block_spec);
        posix_script_builder.comment(comment);
        let (mut coinbase_txouts, txs) = gen_txs(&block_spec);
        let addr = Address::p2wsh(&unlocked_script(), network);
        let coinbase_value_txout = TxOut {
            value: block_subsidy(network, height),
            script_pubkey: addr.script_pubkey(),
        };
        coinbase_txouts.push(coinbase_value_txout);
        let block =
            gen_block(prev_blockhash, target, height, coinbase_txouts, txs)?;
        posix_script_builder.submitblock(&block);
        height += 1;
        prev_blockhash = block.block_hash();
        target = block.header.target().to_compact_lossy();
    }
    println!("{}", posix_script_builder.finalize());
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    gen_script(
        cli.network.into(),
        cli.rpc_addr,
        cli.rpc_auth,
        cli.blocks_spec,
    )
    .await
}
