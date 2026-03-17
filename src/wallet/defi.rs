/// DeFi protocol interfaces for Genesis wallet
///
/// Decentralized swap interface, liquidity pool management,
/// yield farming strategies, staking positions, price oracle
/// feeds. All value calculations use Q16 fixed-point math.

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
// Token / Pool identifiers
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum TokenId {
    Native,          // chain native token (ETH, SOL, etc.)
    Stablecoin,      // USDC/USDT/DAI
    WrappedBtc,
    GovernanceA,
    GovernanceB,
    LpToken(u32),    // liquidity provider token for pool id
    Custom(u32),
}

#[derive(Clone, Copy, PartialEq)]
pub enum DexProtocol {
    UniswapV2,
    UniswapV3,
    Curve,
    Balancer,
    Custom(u32),
}

#[derive(Clone, Copy, PartialEq)]
pub enum StakeStatus {
    Active,
    Unbonding,
    Withdrawn,
    Slashed,
}

#[derive(Clone, Copy, PartialEq)]
pub enum FarmStatus {
    Active,
    Paused,
    Harvested,
    Exited,
}

// ---------------------------------------------------------------------------
// Swap
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct SwapQuote {
    token_in: TokenId,
    token_out: TokenId,
    amount_in_atomic: u64,
    amount_out_atomic: u64,
    price_impact_q16: i32,    // Q16 percentage (e.g. 1% = Q16_ONE / 100)
    fee_bps: u32,             // basis points (e.g. 30 = 0.3%)
    route_hops: u8,
    expires_at: u64,
}

#[derive(Clone, Copy)]
struct SwapRecord {
    id: u32,
    protocol: DexProtocol,
    token_in: TokenId,
    token_out: TokenId,
    amount_in: u64,
    amount_out: u64,
    fee_paid: u64,
    slippage_q16: i32,        // actual slippage Q16
    timestamp: u64,
}

// ---------------------------------------------------------------------------
// Liquidity pools
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct LiquidityPool {
    id: u32,
    protocol: DexProtocol,
    token_a: TokenId,
    token_b: TokenId,
    reserve_a: u64,
    reserve_b: u64,
    total_lp_supply: u64,
    fee_bps: u32,
    apy_q16: i32,              // Q16 annual percentage yield
    tvl_usd_cents: u64,        // total value locked
}

#[derive(Clone, Copy)]
struct LpPosition {
    id: u32,
    pool_id: u32,
    lp_tokens: u64,
    deposited_a: u64,
    deposited_b: u64,
    entry_price_q16: i32,      // Q16 ratio of token_a/token_b at entry
    accumulated_fees_a: u64,
    accumulated_fees_b: u64,
    timestamp: u64,
}

// ---------------------------------------------------------------------------
// Yield farming
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct FarmPosition {
    id: u32,
    pool_id: u32,
    staked_lp_tokens: u64,
    reward_token: TokenId,
    pending_rewards: u64,
    reward_rate_q16: i32,       // Q16 tokens per block
    status: FarmStatus,
    start_block: u64,
    last_harvest_block: u64,
}

// ---------------------------------------------------------------------------
// Staking
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct StakePosition {
    id: u32,
    validator_hash: u64,
    token: TokenId,
    amount_atomic: u64,
    rewards_earned: u64,
    apy_q16: i32,               // Q16 annual percentage yield
    status: StakeStatus,
    lock_until: u64,
    start_timestamp: u64,
    last_claim: u64,
}

// ---------------------------------------------------------------------------
// Price oracle
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct PriceFeed {
    token: TokenId,
    price_usd_q16: i32,        // Q16 USD price (e.g. ETH at $3000 = 3000 * Q16_ONE)
    last_update: u64,
    confidence_q16: i32,        // Q16 confidence (0..Q16_ONE)
    source_count: u8,
    high_24h_q16: i32,
    low_24h_q16: i32,
    change_24h_q16: i32,        // Q16 percentage change
}

impl PriceFeed {
    fn is_stale(&self, now: u64) -> bool {
        now.saturating_sub(self.last_update) > 300 // 5 minutes
    }

    fn spread_q16(&self) -> i32 {
        self.high_24h_q16.saturating_sub(self.low_24h_q16)
    }
}

// ---------------------------------------------------------------------------
// DeFi engine
// ---------------------------------------------------------------------------

struct DefiEngine {
    swap_history: Vec<SwapRecord>,
    pools: Vec<LiquidityPool>,
    lp_positions: Vec<LpPosition>,
    farm_positions: Vec<FarmPosition>,
    stake_positions: Vec<StakePosition>,
    price_feeds: Vec<PriceFeed>,
    next_swap_id: u32,
    next_pool_id: u32,
    next_position_id: u32,
    max_slippage_q16: i32,       // Q16 max acceptable slippage
    total_fees_paid_usd: u64,
}

static DEFI: Mutex<Option<DefiEngine>> = Mutex::new(None);

impl DefiEngine {
    fn new() -> Self {
        DefiEngine {
            swap_history: Vec::new(),
            pools: Vec::new(),
            lp_positions: Vec::new(),
            farm_positions: Vec::new(),
            stake_positions: Vec::new(),
            price_feeds: Vec::new(),
            next_swap_id: 1,
            next_pool_id: 1,
            next_position_id: 1,
            max_slippage_q16: Q16_ONE / 100,  // 1% default
            total_fees_paid_usd: 0,
        }
    }

    // -- Swap --

    fn quote_swap(&self, token_in: TokenId, token_out: TokenId, amount_in: u64) -> Option<SwapQuote> {
        // Find the best pool for this pair
        let pool = self.pools.iter().find(|p| {
            (p.token_a == token_in && p.token_b == token_out)
                || (p.token_b == token_in && p.token_a == token_out)
        })?;

        let (reserve_in, reserve_out) = if pool.token_a == token_in {
            (pool.reserve_a, pool.reserve_b)
        } else {
            (pool.reserve_b, pool.reserve_a)
        };

        if reserve_in == 0 || reserve_out == 0 { return None; }

        // Constant product formula: amount_out = reserve_out * amount_in / (reserve_in + amount_in)
        let fee_amount = amount_in * pool.fee_bps as u64 / 10000;
        let amount_in_after_fee = amount_in.saturating_sub(fee_amount);
        let numerator = reserve_out as u128 * amount_in_after_fee as u128;
        let denominator = reserve_in as u128 + amount_in_after_fee as u128;
        let amount_out = (numerator / denominator) as u64;

        // Price impact = 1 - (amount_out * reserve_in) / (amount_in * reserve_out)
        let ideal_out = (reserve_out as u128 * amount_in as u128 / reserve_in as u128) as u64;
        let impact = if ideal_out > 0 {
            q16_div(
                (ideal_out.saturating_sub(amount_out)) as i32,
                ideal_out as i32,
            )
        } else {
            0
        };

        Some(SwapQuote {
            token_in,
            token_out,
            amount_in_atomic: amount_in,
            amount_out_atomic: amount_out,
            price_impact_q16: impact,
            fee_bps: pool.fee_bps,
            route_hops: 1,
            expires_at: 0,
        })
    }

    fn execute_swap(&mut self, quote: &SwapQuote, protocol: DexProtocol, timestamp: u64) -> Option<u32> {
        if quote.price_impact_q16 > self.max_slippage_q16 {
            return None; // slippage too high
        }

        let id = self.next_swap_id;
        self.next_swap_id = self.next_swap_id.saturating_add(1);

        // Update pool reserves
        if let Some(pool) = self.pools.iter_mut().find(|p| {
            (p.token_a == quote.token_in && p.token_b == quote.token_out)
                || (p.token_b == quote.token_in && p.token_a == quote.token_out)
        }) {
            if pool.token_a == quote.token_in {
                pool.reserve_a += quote.amount_in_atomic;
                pool.reserve_b = pool.reserve_b.saturating_sub(quote.amount_out_atomic);
            } else {
                pool.reserve_b += quote.amount_in_atomic;
                pool.reserve_a = pool.reserve_a.saturating_sub(quote.amount_out_atomic);
            }
        }

        self.swap_history.push(SwapRecord {
            id,
            protocol,
            token_in: quote.token_in,
            token_out: quote.token_out,
            amount_in: quote.amount_in_atomic,
            amount_out: quote.amount_out_atomic,
            fee_paid: quote.amount_in_atomic * quote.fee_bps as u64 / 10000,
            slippage_q16: quote.price_impact_q16,
            timestamp,
        });

        Some(id)
    }

    // -- Liquidity --

    fn add_pool(&mut self, protocol: DexProtocol, token_a: TokenId, token_b: TokenId, fee_bps: u32) -> u32 {
        let id = self.next_pool_id;
        self.next_pool_id = self.next_pool_id.saturating_add(1);
        self.pools.push(LiquidityPool {
            id, protocol, token_a, token_b,
            reserve_a: 0, reserve_b: 0,
            total_lp_supply: 0, fee_bps,
            apy_q16: 0, tvl_usd_cents: 0,
        });
        id
    }

    fn provide_liquidity(&mut self, pool_id: u32, amount_a: u64, amount_b: u64, timestamp: u64) -> Option<u32> {
        let pool = self.pools.iter_mut().find(|p| p.id == pool_id)?;

        // Calculate LP tokens to mint
        let lp_tokens = if pool.total_lp_supply == 0 {
            // Initial liquidity: geometric mean
            let product = (amount_a as u128) * (amount_b as u128);
            let mut sqrt_approx = (product / 2) as u64;
            if sqrt_approx > 0 {
                for _ in 0..32 {
                    let next = (sqrt_approx as u128 + product / sqrt_approx as u128) / 2;
                    sqrt_approx = next as u64;
                }
            }
            sqrt_approx
        } else {
            // Proportional: min(amount_a * supply / reserve_a, amount_b * supply / reserve_b)
            let lp_a = if pool.reserve_a > 0 {
                amount_a as u128 * pool.total_lp_supply as u128 / pool.reserve_a as u128
            } else { 0 };
            let lp_b = if pool.reserve_b > 0 {
                amount_b as u128 * pool.total_lp_supply as u128 / pool.reserve_b as u128
            } else { 0 };
            lp_a.min(lp_b) as u64
        };

        let entry_price = if amount_b > 0 {
            q16_div(amount_a as i32, amount_b as i32)
        } else { 0 };

        pool.reserve_a += amount_a;
        pool.reserve_b += amount_b;
        pool.total_lp_supply += lp_tokens;

        let pos_id = self.next_position_id;
        self.next_position_id = self.next_position_id.saturating_add(1);
        self.lp_positions.push(LpPosition {
            id: pos_id, pool_id, lp_tokens,
            deposited_a: amount_a, deposited_b: amount_b,
            entry_price_q16: entry_price,
            accumulated_fees_a: 0, accumulated_fees_b: 0,
            timestamp,
        });
        Some(pos_id)
    }

    fn withdraw_liquidity(&mut self, position_id: u32) -> Option<(u64, u64)> {
        let pos_idx = self.lp_positions.iter().position(|p| p.id == position_id)?;
        let pos = self.lp_positions[pos_idx];

        let pool = self.pools.iter_mut().find(|p| p.id == pos.pool_id)?;
        if pool.total_lp_supply == 0 { return None; }

        let amount_a = (pos.lp_tokens as u128 * pool.reserve_a as u128
            / pool.total_lp_supply as u128) as u64;
        let amount_b = (pos.lp_tokens as u128 * pool.reserve_b as u128
            / pool.total_lp_supply as u128) as u64;

        pool.reserve_a = pool.reserve_a.saturating_sub(amount_a);
        pool.reserve_b = pool.reserve_b.saturating_sub(amount_b);
        pool.total_lp_supply = pool.total_lp_supply.saturating_sub(pos.lp_tokens);

        self.lp_positions.remove(pos_idx);
        Some((amount_a + pos.accumulated_fees_a, amount_b + pos.accumulated_fees_b))
    }

    // -- Staking --

    fn stake(&mut self, validator_hash: u64, token: TokenId, amount: u64, lock_until: u64, apy_q16: i32, timestamp: u64) -> u32 {
        let id = self.next_position_id;
        self.next_position_id = self.next_position_id.saturating_add(1);
        self.stake_positions.push(StakePosition {
            id, validator_hash, token, amount_atomic: amount,
            rewards_earned: 0, apy_q16,
            status: StakeStatus::Active,
            lock_until, start_timestamp: timestamp,
            last_claim: timestamp,
        });
        id
    }

    fn claim_stake_rewards(&mut self, position_id: u32, current_time: u64) -> u64 {
        if let Some(pos) = self.stake_positions.iter_mut().find(|p| p.id == position_id) {
            if pos.status != StakeStatus::Active { return 0; }
            let elapsed_secs = current_time.saturating_sub(pos.last_claim);
            // rewards = amount * apy * elapsed / seconds_per_year
            let seconds_per_year: i64 = 31_536_000;
            let reward_q16 = q16_mul(pos.amount_atomic as i32, pos.apy_q16);
            let reward = (reward_q16 as i64 * elapsed_secs as i64 / seconds_per_year) as u64;
            pos.rewards_earned += reward;
            pos.last_claim = current_time;
            reward
        } else {
            0
        }
    }

    fn unstake(&mut self, position_id: u32, current_time: u64) -> bool {
        if let Some(pos) = self.stake_positions.iter_mut().find(|p| p.id == position_id) {
            if pos.status != StakeStatus::Active { return false; }
            if current_time < pos.lock_until {
                return false; // still locked
            }
            pos.status = StakeStatus::Unbonding;
            true
        } else {
            false
        }
    }

    // -- Farming --

    fn enter_farm(&mut self, pool_id: u32, lp_tokens: u64, reward_token: TokenId, rate_q16: i32, block: u64) -> u32 {
        let id = self.next_position_id;
        self.next_position_id = self.next_position_id.saturating_add(1);
        self.farm_positions.push(FarmPosition {
            id, pool_id, staked_lp_tokens: lp_tokens,
            reward_token, pending_rewards: 0,
            reward_rate_q16: rate_q16,
            status: FarmStatus::Active,
            start_block: block, last_harvest_block: block,
        });
        id
    }

    fn harvest_farm(&mut self, position_id: u32, current_block: u64) -> u64 {
        if let Some(pos) = self.farm_positions.iter_mut().find(|p| p.id == position_id) {
            if pos.status != FarmStatus::Active { return 0; }
            let blocks_elapsed = current_block.saturating_sub(pos.last_harvest_block);
            let reward = q16_mul(pos.reward_rate_q16, blocks_elapsed as i32) as u64;
            pos.pending_rewards = 0;
            pos.last_harvest_block = current_block;
            pos.status = FarmStatus::Harvested;
            reward
        } else {
            0
        }
    }

    // -- Price oracle --

    fn update_price(&mut self, token: TokenId, price_q16: i32, confidence_q16: i32, timestamp: u64) {
        if let Some(feed) = self.price_feeds.iter_mut().find(|f| f.token == token) {
            // Track 24h high/low
            if price_q16 > feed.high_24h_q16 { feed.high_24h_q16 = price_q16; }
            if price_q16 < feed.low_24h_q16 || feed.low_24h_q16 == 0 {
                feed.low_24h_q16 = price_q16;
            }
            let old_price = feed.price_usd_q16;
            feed.change_24h_q16 = if old_price > 0 {
                q16_div(price_q16 - old_price, old_price)
            } else { 0 };
            feed.price_usd_q16 = price_q16;
            feed.confidence_q16 = confidence_q16;
            feed.last_update = timestamp;
        } else {
            self.price_feeds.push(PriceFeed {
                token,
                price_usd_q16: price_q16,
                last_update: timestamp,
                confidence_q16,
                source_count: 1,
                high_24h_q16: price_q16,
                low_24h_q16: price_q16,
                change_24h_q16: 0,
            });
        }
    }

    fn get_price_q16(&self, token: TokenId) -> Option<i32> {
        self.price_feeds.iter()
            .find(|f| f.token == token)
            .map(|f| f.price_usd_q16)
    }

    fn portfolio_value_usd_q16(&self) -> i32 {
        let mut total: i64 = 0;
        for pos in &self.stake_positions {
            if pos.status == StakeStatus::Active {
                if let Some(price) = self.get_price_q16(pos.token) {
                    total += q16_mul(pos.amount_atomic as i32, price) as i64;
                }
            }
        }
        total as i32
    }

    fn total_staked(&self) -> u64 {
        self.stake_positions.iter()
            .filter(|p| p.status == StakeStatus::Active)
            .map(|p| p.amount_atomic)
            .sum()
    }

    fn total_farming_rewards(&self) -> u64 {
        self.farm_positions.iter()
            .map(|p| p.pending_rewards)
            .sum()
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn get_token_price_q16(token: TokenId) -> Option<i32> {
    let engine = DEFI.lock();
    engine.as_ref().and_then(|e| e.get_price_q16(token))
}

pub fn total_staked_value() -> u64 {
    let engine = DEFI.lock();
    engine.as_ref().map_or(0, |e| e.total_staked())
}

pub fn pool_count() -> usize {
    let engine = DEFI.lock();
    engine.as_ref().map_or(0, |e| e.pools.len())
}

pub fn init() {
    let mut engine = DEFI.lock();
    *engine = Some(DefiEngine::new());
    serial_println!("    Wallet: DeFi protocols (swap, liquidity, staking, oracle) ready");
}
