/// BLAKE3 hash — parallel, very fast tree hashing
///
/// Pure Rust implementation of the BLAKE3 cryptographic hash function.
/// Based on the BLAKE3 specification: https://github.com/BLAKE3-team/BLAKE3-specs
///
/// Key differences from BLAKE2:
///   - Only 7 rounds (vs 10/12 in BLAKE2s/BLAKE2b)
///   - Merkle tree structure for parallelism
///   - 64-byte (512-bit) message blocks (vs 128 in BLAKE2b)
///   - 32-bit words (like BLAKE2s) even for full security
///   - Fixed 32-byte output (extendable via XOF mode)
///
/// Security: 128-bit collision resistance, 256-bit preimage resistance.
use alloc::vec::Vec;

/// BLAKE3 block size in bytes
const BLOCK_LEN: usize = 64;

/// BLAKE3 chunk size in bytes (1024 = 16 blocks)
const CHUNK_LEN: usize = 1024;

/// Number of rounds in BLAKE3
const ROUNDS: usize = 7;

/// BLAKE3 IV (same as SHA-256 IV / BLAKE2s IV)
const IV: [u32; 8] = [
    0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A, 0x510E527F, 0x9B05688C, 0x1F83D9AB, 0x5BE0CD19,
];

// Domain separation flags
const CHUNK_START: u32 = 1 << 0;
const CHUNK_END: u32 = 1 << 1;
const PARENT: u32 = 1 << 2;
const ROOT: u32 = 1 << 3;
const KEYED_HASH: u32 = 1 << 4;
const DERIVE_KEY_CONTEXT: u32 = 1 << 5;
const DERIVE_KEY_MATERIAL: u32 = 1 << 6;

/// BLAKE3 message schedule permutation
/// Unlike BLAKE2 which has 10-12 different permutations,
/// BLAKE3 uses a single fixed permutation applied each round.
const MSG_PERMUTATION: [usize; 16] = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];

/// Quarter-round mixing function G (BLAKE3 variant, 32-bit words)
///
/// Rotation constants for BLAKE3/BLAKE2s: 16, 12, 8, 7
#[inline(always)]
fn g(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize, mx: u32, my: u32) {
    state[a] = state[a].wrapping_add(state[b]).wrapping_add(mx);
    state[d] = (state[d] ^ state[a]).rotate_right(16);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_right(12);
    state[a] = state[a].wrapping_add(state[b]).wrapping_add(my);
    state[d] = (state[d] ^ state[a]).rotate_right(8);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_right(7);
}

/// One full round of BLAKE3 mixing (column + diagonal)
#[inline(always)]
fn round(state: &mut [u32; 16], m: &[u32; 16]) {
    // Column rounds
    g(state, 0, 4, 8, 12, m[0], m[1]);
    g(state, 1, 5, 9, 13, m[2], m[3]);
    g(state, 2, 6, 10, 14, m[4], m[5]);
    g(state, 3, 7, 11, 15, m[6], m[7]);
    // Diagonal rounds
    g(state, 0, 5, 10, 15, m[8], m[9]);
    g(state, 1, 6, 11, 12, m[10], m[11]);
    g(state, 2, 7, 8, 13, m[12], m[13]);
    g(state, 3, 4, 9, 14, m[14], m[15]);
}

/// Permute the message schedule for the next round
fn permute(m: &mut [u32; 16]) {
    let mut permuted = [0u32; 16];
    for i in 0..16 {
        permuted[i] = m[MSG_PERMUTATION[i]];
    }
    *m = permuted;
}

/// BLAKE3 compression function
///
/// Takes a chaining value (8 words), block (16 words), counter, block_len, and flags.
/// Returns the full 16-word state (for root finalization) or the first 8 words (for chaining).
fn compress(
    cv: &[u32; 8],
    block_words: &[u32; 16],
    counter: u64,
    block_len: u32,
    flags: u32,
) -> [u32; 16] {
    // Initialize state
    let mut state = [0u32; 16];
    state[0..8].copy_from_slice(cv);
    state[8] = IV[0];
    state[9] = IV[1];
    state[10] = IV[2];
    state[11] = IV[3];
    state[12] = counter as u32; // counter low
    state[13] = (counter >> 32) as u32; // counter high
    state[14] = block_len;
    state[15] = flags;

    let mut msg = *block_words;

    // 7 rounds of mixing
    for _ in 0..ROUNDS {
        round(&mut state, &msg);
        permute(&mut msg);
    }

    // Feedforward: XOR upper half with lower half, and XOR lower half with CV
    for i in 0..8 {
        state[i] ^= state[i + 8];
        state[i + 8] ^= cv[i];
    }

    state
}

/// Extract the first 8 words from a compression output as a chaining value
fn first_8(state: &[u32; 16]) -> [u32; 8] {
    let mut cv = [0u32; 8];
    cv.copy_from_slice(&state[..8]);
    cv
}

/// Parse a 64-byte block into 16 little-endian u32 words
fn words_from_block(block: &[u8; BLOCK_LEN]) -> [u32; 16] {
    let mut words = [0u32; 16];
    for i in 0..16 {
        words[i] = u32::from_le_bytes([
            block[i * 4],
            block[i * 4 + 1],
            block[i * 4 + 2],
            block[i * 4 + 3],
        ]);
    }
    words
}

/// State for processing a single chunk (up to 1024 bytes = 16 blocks)
struct ChunkState {
    cv: [u32; 8],
    chunk_counter: u64,
    buf: [u8; BLOCK_LEN],
    buf_len: usize,
    blocks_compressed: u8,
    flags: u32,
}

impl ChunkState {
    fn new(key_words: &[u32; 8], chunk_counter: u64, flags: u32) -> Self {
        ChunkState {
            cv: *key_words,
            chunk_counter,
            buf: [0u8; BLOCK_LEN],
            buf_len: 0,
            blocks_compressed: 0,
            flags,
        }
    }

    fn len(&self) -> usize {
        (self.blocks_compressed as usize) * BLOCK_LEN + self.buf_len
    }

    fn start_flag(&self) -> u32 {
        if self.blocks_compressed == 0 {
            CHUNK_START
        } else {
            0
        }
    }

    fn update(&mut self, data: &[u8]) {
        let mut offset = 0;
        while offset < data.len() {
            // If buffer is full, compress it
            if self.buf_len == BLOCK_LEN {
                let block_words = words_from_block(&self.buf);
                let block_flags = self.flags | self.start_flag();
                let state = compress(
                    &self.cv,
                    &block_words,
                    self.chunk_counter,
                    BLOCK_LEN as u32,
                    block_flags,
                );
                self.cv = first_8(&state);
                self.blocks_compressed = self.blocks_compressed.saturating_add(1);
                self.buf = [0u8; BLOCK_LEN];
                self.buf_len = 0;
            }

            let want = BLOCK_LEN - self.buf_len;
            let take = want.min(data.len() - offset);
            self.buf[self.buf_len..self.buf_len + take]
                .copy_from_slice(&data[offset..offset + take]);
            self.buf_len += take;
            offset += take;
        }
    }

    /// Finalize the chunk and return its output (a chaining value or root output)
    fn output(&self) -> Output {
        let block_words = words_from_block(&self.buf);
        let block_flags = self.flags | self.start_flag() | CHUNK_END;
        Output {
            input_cv: self.cv,
            block_words,
            counter: self.chunk_counter,
            block_len: self.buf_len as u32,
            flags: block_flags,
        }
    }
}

/// An output from either a chunk or a parent node.
/// Can produce either a chaining value (non-root) or a root hash.
struct Output {
    input_cv: [u32; 8],
    block_words: [u32; 16],
    counter: u64,
    block_len: u32,
    flags: u32,
}

impl Output {
    /// Compute the chaining value (first 8 words of compression output)
    fn chaining_value(&self) -> [u32; 8] {
        first_8(&compress(
            &self.input_cv,
            &self.block_words,
            self.counter,
            self.block_len,
            self.flags,
        ))
    }

    /// Produce root hash bytes (extendable output)
    fn root_hash(&self) -> [u8; 32] {
        let state = compress(
            &self.input_cv,
            &self.block_words,
            0, // root always uses counter 0 for basic hash
            self.block_len,
            self.flags | ROOT,
        );
        let mut out = [0u8; 32];
        for i in 0..8 {
            let bytes = state[i].to_le_bytes();
            out[i * 4..i * 4 + 4].copy_from_slice(&bytes);
        }
        out
    }
}

/// Compute the parent output from two child chaining values
fn parent_output(
    left_cv: &[u32; 8],
    right_cv: &[u32; 8],
    key_words: &[u32; 8],
    flags: u32,
) -> Output {
    let mut block_words = [0u32; 16];
    block_words[..8].copy_from_slice(left_cv);
    block_words[8..16].copy_from_slice(right_cv);
    Output {
        input_cv: *key_words,
        block_words,
        counter: 0,
        block_len: BLOCK_LEN as u32,
        flags: flags | PARENT,
    }
}

/// Compute the parent chaining value
fn parent_cv(
    left_cv: &[u32; 8],
    right_cv: &[u32; 8],
    key_words: &[u32; 8],
    flags: u32,
) -> [u32; 8] {
    parent_output(left_cv, right_cv, key_words, flags).chaining_value()
}

/// BLAKE3 hasher state with Merkle tree
pub struct Blake3Hasher {
    /// Stack of chaining values (Merkle tree)
    cv_stack: Vec<[u32; 8]>,
    /// Number of CVs on the stack
    cv_stack_len: usize,
    /// Current chunk state
    chunk_state: ChunkState,
    /// Key words (IV for unkeyed, derived for keyed)
    key_words: [u32; 8],
    /// Domain separation flags
    flags: u32,
}

impl Blake3Hasher {
    /// Create a new BLAKE3 hasher (unkeyed mode)
    pub fn new() -> Self {
        Blake3Hasher {
            cv_stack: Vec::new(),
            cv_stack_len: 0,
            chunk_state: ChunkState::new(&IV, 0, 0),
            key_words: IV,
            flags: 0,
        }
    }

    /// Create a new BLAKE3 hasher in keyed mode
    pub fn new_keyed(key: &[u8; 32]) -> Self {
        let mut key_words = [0u32; 8];
        for i in 0..8 {
            key_words[i] =
                u32::from_le_bytes([key[i * 4], key[i * 4 + 1], key[i * 4 + 2], key[i * 4 + 3]]);
        }
        Blake3Hasher {
            cv_stack: Vec::new(),
            cv_stack_len: 0,
            chunk_state: ChunkState::new(&key_words, 0, KEYED_HASH),
            key_words,
            flags: KEYED_HASH,
        }
    }

    /// Create a BLAKE3 hasher for key derivation
    pub fn new_derive_key(context: &[u8]) -> Self {
        let mut context_hasher = Blake3Hasher {
            cv_stack: Vec::new(),
            cv_stack_len: 0,
            chunk_state: ChunkState::new(&IV, 0, DERIVE_KEY_CONTEXT),
            key_words: IV,
            flags: DERIVE_KEY_CONTEXT,
        };
        context_hasher.update(context);
        let context_key = context_hasher.finalize();
        let mut key_words = [0u32; 8];
        for i in 0..8 {
            key_words[i] = u32::from_le_bytes([
                context_key[i * 4],
                context_key[i * 4 + 1],
                context_key[i * 4 + 2],
                context_key[i * 4 + 3],
            ]);
        }
        Blake3Hasher {
            cv_stack: Vec::new(),
            cv_stack_len: 0,
            chunk_state: ChunkState::new(&key_words, 0, DERIVE_KEY_MATERIAL),
            key_words,
            flags: DERIVE_KEY_MATERIAL,
        }
    }

    /// Feed data into the hasher
    pub fn update(&mut self, data: &[u8]) {
        let mut offset = 0;
        while offset < data.len() {
            // If the current chunk is full, finalize it and push the CV
            if self.chunk_state.len() == CHUNK_LEN {
                let chunk_cv = self.chunk_state.output().chaining_value();
                let total_chunks = self.chunk_state.chunk_counter + 1;

                // Merge complete subtrees
                self.add_chunk_cv(chunk_cv, total_chunks);

                self.chunk_state = ChunkState::new(&self.key_words, total_chunks, self.flags);
            }

            let want = CHUNK_LEN - self.chunk_state.len();
            let take = want.min(data.len() - offset);
            self.chunk_state.update(&data[offset..offset + take]);
            offset += take;
        }
    }

    /// Add a completed chunk's CV to the tree, merging subtrees as needed
    fn add_chunk_cv(&mut self, mut new_cv: [u32; 8], mut total_chunks: u64) {
        // Merge complete subtrees based on the binary representation of total_chunks
        while total_chunks & 1 == 0 && self.cv_stack_len > 0 {
            self.cv_stack_len -= 1;
            let left = self.cv_stack[self.cv_stack_len];
            new_cv = parent_cv(&left, &new_cv, &self.key_words, self.flags);
            total_chunks >>= 1;
        }

        // Push the (possibly merged) CV
        if self.cv_stack_len < self.cv_stack.len() {
            self.cv_stack[self.cv_stack_len] = new_cv;
        } else {
            self.cv_stack.push(new_cv);
        }
        self.cv_stack_len += 1;
    }

    /// Finalize the hash and return a 32-byte digest
    pub fn finalize(self) -> [u8; 32] {
        // Get the output from the current (possibly partial) chunk
        let mut output = self.chunk_state.output();

        // Merge all remaining CVs on the stack from right to left
        let mut parent_nodes_remaining = self.cv_stack_len;
        while parent_nodes_remaining > 0 {
            parent_nodes_remaining -= 1;
            let left = self.cv_stack[parent_nodes_remaining];
            let right_cv = output.chaining_value();
            output = parent_output(&left, &right_cv, &self.key_words, self.flags);
        }

        output.root_hash()
    }
}

/// One-shot BLAKE3 hash
pub fn blake3_hash(data: &[u8]) -> [u8; 32] {
    let mut hasher = Blake3Hasher::new();
    hasher.update(data);
    hasher.finalize()
}

/// One-shot BLAKE3 keyed hash (MAC)
pub fn blake3_keyed_hash(key: &[u8; 32], data: &[u8]) -> [u8; 32] {
    let mut hasher = Blake3Hasher::new_keyed(key);
    hasher.update(data);
    hasher.finalize()
}

/// One-shot BLAKE3 key derivation
pub fn blake3_derive_key(context: &[u8], key_material: &[u8]) -> [u8; 32] {
    let mut hasher = Blake3Hasher::new_derive_key(context);
    hasher.update(key_material);
    hasher.finalize()
}

pub fn init() {
    crate::serial_println!("    [blake3] BLAKE3 (hash, keyed, KDF) ready");
}
