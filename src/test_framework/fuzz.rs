use crate::sync::Mutex;
/// Fuzz testing harness
///
/// Part of the AIOS. Generates random inputs to test
/// kernel interfaces for crashes and unexpected behavior.
/// Uses a simple xorshift64 PRNG for deterministic fuzzing.
use alloc::vec::Vec;

/// Xorshift64 PRNG step — fast, deterministic, no_std compatible
fn xorshift64(state: &mut u64) -> u64 {
    let mut x = *state;
    if x == 0 {
        x = 0xDEAD_BEEF_CAFE_BABE;
    } // avoid zero state
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

/// Generates random inputs to test kernel interfaces for crashes.
pub struct FuzzHarness {
    seed: u64,
    corpus: Vec<Vec<u8>>,
}

/// Result of a fuzz campaign
pub struct FuzzResult {
    pub iterations_run: u64,
    pub crashes: u64,
    pub unique_paths: usize,
}

impl FuzzHarness {
    pub fn new(seed: u64) -> Self {
        let effective_seed = if seed == 0 {
            0xCAFE_1234_DEAD_5678
        } else {
            seed
        };
        crate::serial_println!("    [fuzz] harness created with seed={:#x}", effective_seed);
        Self {
            seed: effective_seed,
            corpus: Vec::new(),
        }
    }

    /// Generate a random input buffer of up to max_len bytes.
    /// Uses the internal PRNG seeded at construction.
    pub fn generate_input(&mut self, max_len: usize) -> Vec<u8> {
        // Determine length: 1..=max_len
        let len_raw = xorshift64(&mut self.seed);
        let len = if max_len == 0 {
            1
        } else {
            ((len_raw % max_len as u64) + 1) as usize
        };

        let mut buf = Vec::with_capacity(len);
        for _ in 0..len {
            let byte = (xorshift64(&mut self.seed) & 0xFF) as u8;
            buf.push(byte);
        }
        buf
    }

    /// Generate a biased input that includes interesting byte patterns.
    /// Mixes random bytes with edge-case values (0, 0xFF, boundaries).
    pub fn generate_biased_input(&mut self, max_len: usize) -> Vec<u8> {
        let mut buf = self.generate_input(max_len);

        // Sprinkle in interesting values at random positions
        let interesting_bytes: &[u8] = &[0x00, 0xFF, 0x7F, 0x80, 0x01, 0xFE];
        let inject_count = if buf.len() > 4 { buf.len() / 4 } else { 1 };

        for _ in 0..inject_count {
            let pos = (xorshift64(&mut self.seed) as usize) % buf.len();
            let val_idx = (xorshift64(&mut self.seed) as usize) % interesting_bytes.len();
            buf[pos] = interesting_bytes[val_idx];
        }

        buf
    }

    /// Add an input to the corpus for later mutation-based fuzzing
    pub fn add_to_corpus(&mut self, input: Vec<u8>) {
        self.corpus.push(input);
    }

    /// Mutate an existing corpus entry
    fn mutate_corpus_entry(&mut self) -> Option<Vec<u8>> {
        if self.corpus.is_empty() {
            return None;
        }
        let idx = (xorshift64(&mut self.seed) as usize) % self.corpus.len();
        let mut mutated = self.corpus[idx].clone();

        if mutated.is_empty() {
            mutated.push((xorshift64(&mut self.seed) & 0xFF) as u8);
            return Some(mutated);
        }

        // Apply random mutation
        let mutation_type = xorshift64(&mut self.seed) % 4;
        match mutation_type {
            0 => {
                // Bit flip
                let pos = (xorshift64(&mut self.seed) as usize) % mutated.len();
                let bit = (xorshift64(&mut self.seed) % 8) as u8;
                mutated[pos] ^= 1 << bit;
            }
            1 => {
                // Byte replacement
                let pos = (xorshift64(&mut self.seed) as usize) % mutated.len();
                mutated[pos] = (xorshift64(&mut self.seed) & 0xFF) as u8;
            }
            2 => {
                // Insert a byte
                if mutated.len() < 4096 {
                    let pos = (xorshift64(&mut self.seed) as usize) % (mutated.len() + 1);
                    let val = (xorshift64(&mut self.seed) & 0xFF) as u8;
                    mutated.insert(pos, val);
                }
            }
            _ => {
                // Delete a byte
                if mutated.len() > 1 {
                    let pos = (xorshift64(&mut self.seed) as usize) % mutated.len();
                    mutated.remove(pos);
                }
            }
        }

        Some(mutated)
    }

    /// Run a fuzz target with generated inputs.
    /// Calls the target function with random inputs for the given number
    /// of iterations. The target should not panic on any input.
    pub fn fuzz(&mut self, target: fn(&[u8]), iterations: u64) {
        crate::serial_println!(
            "    [fuzz] starting fuzz campaign: {} iterations",
            iterations
        );

        let mut crashes = 0u64;

        for i in 0..iterations {
            // Alternate between fresh random, biased, and corpus mutation
            let input = match i % 3 {
                0 => self.generate_input(256),
                1 => self.generate_biased_input(256),
                _ => match self.mutate_corpus_entry() {
                    Some(mutated) => mutated,
                    None => self.generate_input(256),
                },
            };

            // Run the target (in a real kernel we'd wrap this with a fault handler)
            target(&input);

            // If we get here, the target survived this input
            // Add interesting inputs to corpus (every 10th)
            if i % 10 == 0 && self.corpus.len() < 1024 {
                self.add_to_corpus(input);
            }
        }

        crate::serial_println!(
            "    [fuzz] campaign complete: {} iterations, {} crashes, {} corpus entries",
            iterations,
            crashes,
            self.corpus.len()
        );
    }

    /// Get the current corpus size
    pub fn corpus_size(&self) -> usize {
        self.corpus.len()
    }

    /// Reset the harness with a new seed
    pub fn reseed(&mut self, new_seed: u64) {
        self.seed = if new_seed == 0 {
            0xCAFE_1234_DEAD_5678
        } else {
            new_seed
        };
        crate::serial_println!("    [fuzz] reseeded with {:#x}", self.seed);
    }

    /// Clear the corpus
    pub fn clear_corpus(&mut self) {
        self.corpus.clear();
        crate::serial_println!("    [fuzz] corpus cleared");
    }
}

/// Global fuzz harness singleton
static FUZZ_HARNESS: Mutex<Option<FuzzHarness>> = Mutex::new(None);

pub fn init() {
    let mut harness = FUZZ_HARNESS.lock();
    *harness = Some(FuzzHarness::new(0xABCD_EF01_2345_6789));
    crate::serial_println!("    [fuzz] fuzz testing subsystem initialized");
}
