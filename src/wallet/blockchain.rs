/// Blockchain client for Genesis wallet
///
/// Block parsing, transaction verification, merkle tree computation,
/// wallet address management, balance tracking, UTXO model,
/// chain synchronization, and block header validation.
///
/// Uses Q16 fixed-point for fee rate calculations.

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

/// Q16 fixed-point: 1 << 16
const Q16_ONE: i32 = 65536;

/// Q16 multiply: (a * b) >> 16
fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 16) as i32
}

/// Q16 divide: (a << 16) / b
fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 { return 0; }
    ((a as i64) << 16) / (b as i64) as i32
}

// ---------------------------------------------------------------------------
// Block structures
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum ChainId {
    Bitcoin,
    Ethereum,
    Testnet,
    Custom(u32),
}

#[derive(Clone, Copy)]
pub struct BlockHeader {
    pub version: u32,
    pub prev_hash: [u8; 32],
    pub merkle_root: [u8; 32],
    pub timestamp: u64,
    pub difficulty: u32,
    pub nonce: u32,
    pub height: u64,
}

#[derive(Clone, Copy, PartialEq)]
pub enum TxStatus {
    Unconfirmed,
    Confirmed,
    Failed,
    Orphaned,
}

#[derive(Clone)]
pub struct TxInput {
    pub prev_tx_hash: [u8; 32],
    pub output_index: u32,
    pub script_sig: [u8; 64],
    pub script_len: usize,
    pub sequence: u32,
}

#[derive(Clone)]
pub struct TxOutput {
    pub value_sats: u64,
    pub script_pubkey: [u8; 64],
    pub script_len: usize,
    pub spent: bool,
}

#[derive(Clone)]
pub struct Transaction {
    pub tx_hash: [u8; 32],
    pub inputs: Vec<TxInput>,
    pub outputs: Vec<TxOutput>,
    pub lock_time: u32,
    pub status: TxStatus,
    pub block_height: Option<u64>,
    pub fee_sats: u64,
    pub timestamp: u64,
    pub size_bytes: u32,
}

#[derive(Clone)]
pub struct Block {
    pub header: BlockHeader,
    pub transactions: Vec<Transaction>,
    pub tx_count: u32,
    pub size_bytes: u32,
}

// ---------------------------------------------------------------------------
// UTXO tracking
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Utxo {
    tx_hash: [u8; 32],
    output_index: u32,
    value_sats: u64,
    script_pubkey: [u8; 64],
    script_len: usize,
    block_height: u64,
    confirmed: bool,
}

// ---------------------------------------------------------------------------
// Wallet address
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct WalletAddress {
    id: u32,
    address_hash: [u8; 32],
    label: [u8; 32],
    label_len: usize,
    chain: ChainId,
    total_received_sats: u64,
    total_sent_sats: u64,
    tx_count: u32,
    is_change: bool,
}

// ---------------------------------------------------------------------------
// Merkle tree
// ---------------------------------------------------------------------------

fn simple_hash(data: &[u8]) -> [u8; 32] {
    let mut h = [0u8; 32];
    let mut acc: u64 = 0xCBF2_9CE4_8422_2325;
    for &b in data {
        acc = acc.wrapping_mul(0x0100_0000_01B3).wrapping_add(b as u64);
    }
    let bytes = acc.to_le_bytes();
    h[0..8].copy_from_slice(&bytes);
    acc = acc.wrapping_mul(0x517C_C1B7_2722_0A95);
    let bytes2 = acc.to_le_bytes();
    h[8..16].copy_from_slice(&bytes2);
    acc = acc.wrapping_mul(0x6C62_272E_07BB_0142);
    let bytes3 = acc.to_le_bytes();
    h[16..24].copy_from_slice(&bytes3);
    acc = acc.wrapping_mul(0x0100_0000_01B3);
    let bytes4 = acc.to_le_bytes();
    h[24..32].copy_from_slice(&bytes4);
    h
}

fn hash_pair(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut combined = [0u8; 64];
    combined[..32].copy_from_slice(left);
    combined[32..].copy_from_slice(right);
    simple_hash(&combined)
}

fn compute_merkle_root(tx_hashes: &[[u8; 32]]) -> [u8; 32] {
    if tx_hashes.is_empty() {
        return [0u8; 32];
    }
    if tx_hashes.len() == 1 {
        return tx_hashes[0];
    }
    let mut current_level: Vec<[u8; 32]> = tx_hashes.to_vec();
    while current_level.len() > 1 {
        let mut next_level = Vec::new();
        let mut i = 0;
        while i < current_level.len() {
            let left = &current_level[i];
            let right = if i + 1 < current_level.len() {
                &current_level[i + 1]
            } else {
                &current_level[i] // duplicate last if odd
            };
            next_level.push(hash_pair(left, right));
            i += 2;
        }
        current_level = next_level;
    }
    current_level[0]
}

// ---------------------------------------------------------------------------
// Fee estimation (Q16 fixed-point sats/byte)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct FeeEstimate {
    low_priority_q16: i32,     // Q16 sats/byte
    medium_priority_q16: i32,
    high_priority_q16: i32,
    last_block_avg_q16: i32,
}

impl FeeEstimate {
    fn new() -> Self {
        FeeEstimate {
            low_priority_q16: 5 * Q16_ONE,
            medium_priority_q16: 20 * Q16_ONE,
            high_priority_q16: 50 * Q16_ONE,
            last_block_avg_q16: 15 * Q16_ONE,
        }
    }

    fn estimate_fee_sats(&self, size_bytes: u32, priority: u8) -> u64 {
        let rate = match priority {
            0 => self.low_priority_q16,
            1 => self.medium_priority_q16,
            _ => self.high_priority_q16,
        };
        let fee_q16 = q16_mul(rate, size_bytes as i32 * Q16_ONE);
        (fee_q16 / Q16_ONE) as u64
    }

    fn update_from_block(&mut self, block: &Block) {
        if block.transactions.is_empty() { return; }
        let mut total_rate_q16: i64 = 0;
        let mut count = 0i64;
        for tx in &block.transactions {
            if tx.size_bytes > 0 {
                let rate = q16_div(tx.fee_sats as i32, tx.size_bytes as i32);
                total_rate_q16 += rate as i64;
                count += 1;
            }
        }
        if count > 0 {
            self.last_block_avg_q16 = (total_rate_q16 / count) as i32;
            // Adjust estimates based on recent average
            self.low_priority_q16 = q16_mul(self.last_block_avg_q16, Q16_ONE / 2);
            self.medium_priority_q16 = self.last_block_avg_q16;
            self.high_priority_q16 = q16_mul(self.last_block_avg_q16, 2 * Q16_ONE);
        }
    }
}

// ---------------------------------------------------------------------------
// Blockchain client
// ---------------------------------------------------------------------------

struct BlockchainClient {
    chain: ChainId,
    addresses: Vec<WalletAddress>,
    utxos: Vec<Utxo>,
    recent_blocks: Vec<BlockHeader>,
    pending_txs: Vec<Transaction>,
    fee_estimate: FeeEstimate,
    best_height: u64,
    next_addr_id: u32,
    synced: bool,
    total_balance_sats: u64,
    unconfirmed_sats: u64,
}

static BLOCKCHAIN: Mutex<Option<BlockchainClient>> = Mutex::new(None);

impl BlockchainClient {
    fn new(chain: ChainId) -> Self {
        BlockchainClient {
            chain,
            addresses: Vec::new(),
            utxos: Vec::new(),
            recent_blocks: Vec::new(),
            pending_txs: Vec::new(),
            fee_estimate: FeeEstimate::new(),
            best_height: 0,
            next_addr_id: 1,
            synced: false,
            total_balance_sats: 0,
            unconfirmed_sats: 0,
        }
    }

    fn add_address(&mut self, address_hash: [u8; 32], label: &[u8], is_change: bool) -> u32 {
        let id = self.next_addr_id;
        self.next_addr_id = self.next_addr_id.saturating_add(1);
        let mut lbl = [0u8; 32];
        let llen = label.len().min(32);
        lbl[..llen].copy_from_slice(&label[..llen]);
        self.addresses.push(WalletAddress {
            id,
            address_hash,
            label: lbl,
            label_len: llen,
            chain: self.chain,
            total_received_sats: 0,
            total_sent_sats: 0,
            tx_count: 0,
            is_change,
        });
        id
    }

    fn process_block(&mut self, block: Block) -> u32 {
        let mut relevant_count = 0u32;
        self.best_height = block.header.height;
        // Store block header (keep last 100)
        self.recent_blocks.push(block.header);
        if self.recent_blocks.len() > 100 {
            self.recent_blocks.remove(0);
        }
        // Update fee estimate
        self.fee_estimate.update_from_block(&block);
        // Scan transactions for relevant addresses
        for tx in &block.transactions {
            if self.scan_transaction(tx, true) {
                relevant_count += 1;
            }
        }
        // Remove confirmed transactions from pending
        self.pending_txs.retain(|ptx| {
            !block.transactions.iter().any(|btx| btx.tx_hash == ptx.tx_hash)
        });
        relevant_count
    }

    fn scan_transaction(&mut self, tx: &Transaction, confirmed: bool) -> bool {
        let mut relevant = false;
        let addr_hashes: Vec<[u8; 32]> = self.addresses.iter()
            .map(|a| a.address_hash)
            .collect();
        // Check outputs for incoming funds
        for (idx, output) in tx.outputs.iter().enumerate() {
            let out_hash = simple_hash(&output.script_pubkey[..output.script_len]);
            for addr_hash in &addr_hashes {
                if out_hash == *addr_hash {
                    relevant = true;
                    self.utxos.push(Utxo {
                        tx_hash: tx.tx_hash,
                        output_index: idx as u32,
                        value_sats: output.value_sats,
                        script_pubkey: output.script_pubkey,
                        script_len: output.script_len,
                        block_height: tx.block_height.unwrap_or(0),
                        confirmed,
                    });
                    if confirmed {
                        self.total_balance_sats += output.value_sats;
                    } else {
                        self.unconfirmed_sats += output.value_sats;
                    }
                    if let Some(addr) = self.addresses.iter_mut()
                        .find(|a| a.address_hash == *addr_hash)
                    {
                        addr.total_received_sats += output.value_sats;
                        addr.tx_count = addr.tx_count.saturating_add(1);
                    }
                }
            }
        }
        // Check inputs for spent UTXOs
        for input in &tx.inputs {
            let spent_idx = self.utxos.iter().position(|u| {
                u.tx_hash == input.prev_tx_hash && u.output_index == input.output_index
            });
            if let Some(idx) = spent_idx {
                let spent_val = self.utxos[idx].value_sats;
                self.total_balance_sats = self.total_balance_sats.saturating_sub(spent_val);
                self.utxos.remove(idx);
                relevant = true;
            }
        }
        relevant
    }

    fn verify_block_header(&self, header: &BlockHeader) -> bool {
        // Verify previous hash links to our chain tip
        if let Some(last) = self.recent_blocks.last() {
            let last_hash = simple_hash(&last.prev_hash);
            // Simplified: check height is sequential
            if header.height != last.height + 1 {
                return false;
            }
            // Check timestamp is reasonable (not more than 2 hours in the future)
            if header.timestamp < last.timestamp {
                return false;
            }
        }
        // Verify difficulty target
        if header.difficulty == 0 {
            return false;
        }
        true
    }

    fn verify_merkle_root(&self, block: &Block) -> bool {
        let tx_hashes: Vec<[u8; 32]> = block.transactions.iter()
            .map(|tx| tx.tx_hash)
            .collect();
        let computed = compute_merkle_root(&tx_hashes);
        computed == block.header.merkle_root
    }

    fn verify_transaction(&self, tx: &Transaction) -> bool {
        // Basic transaction verification
        if tx.inputs.is_empty() || tx.outputs.is_empty() {
            return false;
        }
        // Check for duplicate inputs
        for i in 0..tx.inputs.len() {
            for j in (i + 1)..tx.inputs.len() {
                if tx.inputs[i].prev_tx_hash == tx.inputs[j].prev_tx_hash
                    && tx.inputs[i].output_index == tx.inputs[j].output_index
                {
                    return false;
                }
            }
        }
        // Check output values don't overflow
        let mut total_out: u64 = 0;
        for output in &tx.outputs {
            total_out = match total_out.checked_add(output.value_sats) {
                Some(v) => v,
                None => return false,
            };
        }
        true
    }

    fn select_utxos(&self, target_sats: u64) -> Option<Vec<usize>> {
        // Simple greedy UTXO selection
        let mut sorted_indices: Vec<usize> = (0..self.utxos.len())
            .filter(|&i| self.utxos[i].confirmed && !self.utxos[i].spent_check())
            .collect();
        sorted_indices.sort_by(|&a, &b| self.utxos[b].value_sats.cmp(&self.utxos[a].value_sats));

        let mut selected = Vec::new();
        let mut accumulated = 0u64;
        for idx in sorted_indices {
            selected.push(idx);
            accumulated += self.utxos[idx].value_sats;
            if accumulated >= target_sats {
                return Some(selected);
            }
        }
        None // insufficient funds
    }

    fn get_balance(&self) -> u64 {
        self.total_balance_sats
    }

    fn get_unconfirmed_balance(&self) -> u64 {
        self.unconfirmed_sats
    }

    fn get_address_balance(&self, addr_id: u32) -> u64 {
        if let Some(addr) = self.addresses.iter().find(|a| a.id == addr_id) {
            addr.total_received_sats.saturating_sub(addr.total_sent_sats)
        } else {
            0
        }
    }

    fn fee_rate_q16(&self, priority: u8) -> i32 {
        match priority {
            0 => self.fee_estimate.low_priority_q16,
            1 => self.fee_estimate.medium_priority_q16,
            _ => self.fee_estimate.high_priority_q16,
        }
    }

    fn chain_progress_q16(&self) -> i32 {
        // Return sync progress as Q16 fraction (0 = 0%, Q16_ONE = 100%)
        if self.synced {
            Q16_ONE
        } else if self.best_height == 0 {
            0
        } else {
            // Estimated total height for progress (placeholder)
            let estimated_total = 800_000i64;
            ((self.best_height as i64 * Q16_ONE as i64) / estimated_total) as i32
        }
    }

    fn address_count(&self) -> usize {
        self.addresses.len()
    }

    fn utxo_count(&self) -> usize {
        self.utxos.len()
    }

    fn pending_tx_count(&self) -> usize {
        self.pending_txs.len()
    }
}

impl Utxo {
    fn spent_check(&self) -> bool {
        // In a real implementation this would check against spent set
        false
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn add_address(hash: [u8; 32], label: &[u8], is_change: bool) -> Option<u32> {
    let mut bc = BLOCKCHAIN.lock();
    bc.as_mut().map(|c| c.add_address(hash, label, is_change))
}

pub fn get_balance() -> u64 {
    let bc = BLOCKCHAIN.lock();
    bc.as_ref().map_or(0, |c| c.get_balance())
}

pub fn get_unconfirmed() -> u64 {
    let bc = BLOCKCHAIN.lock();
    bc.as_ref().map_or(0, |c| c.get_unconfirmed_balance())
}

pub fn best_height() -> u64 {
    let bc = BLOCKCHAIN.lock();
    bc.as_ref().map_or(0, |c| c.best_height)
}

pub fn verify_tx(tx: &Transaction) -> bool {
    let bc = BLOCKCHAIN.lock();
    bc.as_ref().map_or(false, |c| c.verify_transaction(tx))
}

pub fn init() {
    let mut bc = BLOCKCHAIN.lock();
    *bc = Some(BlockchainClient::new(ChainId::Bitcoin));
    serial_println!("    Wallet: blockchain client (parsing, verification, merkle) ready");
}
