// video_codec/transform.rs - DCT and inverse DCT transforms

#![no_std]

/// Transform operations (DCT, IDCT)
pub struct Transform;

impl Transform {
    /// Inverse DCT 4x4 (used in H.264, H.265, VP9, AV1)
    pub fn inverse_dct_4x4(input: &[i16], output: &mut [i16]) {
        const SCALE: i32 = 64;
        let mut temp = [0i32; 16];

        // Horizontal 1D IDCT
        for i in 0..4 {
            let s0 = input[i * 4 + 0] as i32;
            let s1 = input[i * 4 + 1] as i32;
            let s2 = input[i * 4 + 2] as i32;
            let s3 = input[i * 4 + 3] as i32;

            let t0 = s0 + s2;
            let t1 = s0 - s2;
            let t2 = s1 - s3;
            let t3 = s1 + s3;

            temp[i * 4 + 0] = t0 + t3;
            temp[i * 4 + 1] = t1 + t2;
            temp[i * 4 + 2] = t1 - t2;
            temp[i * 4 + 3] = t0 - t3;
        }

        // Vertical 1D IDCT
        for i in 0..4 {
            let s0 = temp[0 * 4 + i];
            let s1 = temp[1 * 4 + i];
            let s2 = temp[2 * 4 + i];
            let s3 = temp[3 * 4 + i];

            let t0 = s0 + s2;
            let t1 = s0 - s2;
            let t2 = s1 - s3;
            let t3 = s1 + s3;

            output[0 * 4 + i] = ((t0 + t3 + SCALE / 2) / SCALE).clamp(-32768, 32767) as i16;
            output[1 * 4 + i] = ((t1 + t2 + SCALE / 2) / SCALE).clamp(-32768, 32767) as i16;
            output[2 * 4 + i] = ((t1 - t2 + SCALE / 2) / SCALE).clamp(-32768, 32767) as i16;
            output[3 * 4 + i] = ((t0 - t3 + SCALE / 2) / SCALE).clamp(-32768, 32767) as i16;
        }
    }

    /// Forward DCT 4x4
    pub fn forward_dct_4x4(input: &[i16], output: &mut [i16]) {
        let mut temp = [0i32; 16];

        // Horizontal 1D DCT
        for i in 0..4 {
            let s0 = input[i * 4 + 0] as i32;
            let s1 = input[i * 4 + 1] as i32;
            let s2 = input[i * 4 + 2] as i32;
            let s3 = input[i * 4 + 3] as i32;

            let t0 = s0 + s3;
            let t1 = s1 + s2;
            let t2 = s1 - s2;
            let t3 = s0 - s3;

            temp[i * 4 + 0] = t0 + t1;
            temp[i * 4 + 1] = t3 + t2;
            temp[i * 4 + 2] = t0 - t1;
            temp[i * 4 + 3] = t3 - t2;
        }

        // Vertical 1D DCT
        for i in 0..4 {
            let s0 = temp[0 * 4 + i];
            let s1 = temp[1 * 4 + i];
            let s2 = temp[2 * 4 + i];
            let s3 = temp[3 * 4 + i];

            let t0 = s0 + s3;
            let t1 = s1 + s2;
            let t2 = s1 - s2;
            let t3 = s0 - s3;

            output[0 * 4 + i] = ((t0 + t1) >> 1).clamp(-32768, 32767) as i16;
            output[1 * 4 + i] = ((t3 + t2) >> 1).clamp(-32768, 32767) as i16;
            output[2 * 4 + i] = ((t0 - t1) >> 1).clamp(-32768, 32767) as i16;
            output[3 * 4 + i] = ((t3 - t2) >> 1).clamp(-32768, 32767) as i16;
        }
    }

    /// Inverse DCT 8x8
    pub fn inverse_dct_8x8(input: &[i16], output: &mut [i16]) {
        // Simplified 8x8 IDCT
        for i in 0..8 {
            for j in 0..8 {
                let mut sum = 0i32;

                for u in 0..8 {
                    for v in 0..8 {
                        let cu = if u == 0 { 362 } else { 512 }; // 1/sqrt(2) * 512
                        let cv = if v == 0 { 362 } else { 512 };

                        let cos_u = Self::cos_lut(u, i);
                        let cos_v = Self::cos_lut(v, j);

                        sum += input[u * 8 + v] as i32 * cu * cv * cos_u * cos_v;
                    }
                }

                output[i * 8 + j] = ((sum + 131072) >> 18).clamp(-32768, 32767) as i16;
            }
        }
    }

    /// Inverse DCT 16x16
    pub fn inverse_dct_16x16(input: &[i16], output: &mut [i16]) {
        // Simplified: use 4x4 IDCT on sub-blocks
        for block_y in 0..4 {
            for block_x in 0..4 {
                let mut block_in = [0i16; 16];
                let mut block_out = [0i16; 16];

                for i in 0..4 {
                    for j in 0..4 {
                        block_in[i * 4 + j] = input[(block_y * 4 + i) * 16 + (block_x * 4 + j)];
                    }
                }

                Self::inverse_dct_4x4(&block_in, &mut block_out);

                for i in 0..4 {
                    for j in 0..4 {
                        output[(block_y * 4 + i) * 16 + (block_x * 4 + j)] = block_out[i * 4 + j];
                    }
                }
            }
        }
    }

    /// Inverse DCT 32x32
    pub fn inverse_dct_32x32(input: &[i16], output: &mut [i16]) {
        // Simplified: use 4x4 IDCT on sub-blocks
        for block_y in 0..8 {
            for block_x in 0..8 {
                let mut block_in = [0i16; 16];
                let mut block_out = [0i16; 16];

                for i in 0..4 {
                    for j in 0..4 {
                        let src_y = block_y * 4 + i;
                        let src_x = block_x * 4 + j;
                        if src_y < 32 && src_x < 32 {
                            block_in[i * 4 + j] = input[src_y * 32 + src_x];
                        }
                    }
                }

                Self::inverse_dct_4x4(&block_in, &mut block_out);

                for i in 0..4 {
                    for j in 0..4 {
                        let dst_y = block_y * 4 + i;
                        let dst_x = block_x * 4 + j;
                        if dst_y < 32 && dst_x < 32 {
                            output[dst_y * 32 + dst_x] = block_out[i * 4 + j];
                        }
                    }
                }
            }
        }
    }

    /// Inverse DCT 64x64 (AV1)
    pub fn inverse_dct_64x64(input: &[i16], output: &mut [i16]) {
        // Simplified: use 4x4 IDCT on sub-blocks
        for block_y in 0..16 {
            for block_x in 0..16 {
                let mut block_in = [0i16; 16];
                let mut block_out = [0i16; 16];

                for i in 0..4 {
                    for j in 0..4 {
                        let src_y = block_y * 4 + i;
                        let src_x = block_x * 4 + j;
                        if src_y < 64 && src_x < 64 {
                            block_in[i * 4 + j] = input[src_y * 64 + src_x];
                        }
                    }
                }

                Self::inverse_dct_4x4(&block_in, &mut block_out);

                for i in 0..4 {
                    for j in 0..4 {
                        let dst_y = block_y * 4 + i;
                        let dst_x = block_x * 4 + j;
                        if dst_y < 64 && dst_x < 64 {
                            output[dst_y * 64 + dst_x] = block_out[i * 4 + j];
                        }
                    }
                }
            }
        }
    }

    /// Cosine lookup table for 8x8 DCT
    fn cos_lut(k: usize, n: usize) -> i32 {
        // Simplified cosine values (scaled by 512)
        const COS_TABLE: [[i32; 8]; 8] = [
            [512, 512, 512, 512, 512, 512, 512, 512],
            [502, 426, 284, 100, -100, -284, -426, -502],
            [473, 196, -196, -473, -473, -196, 196, 473],
            [426, -100, -502, -284, 284, 502, 100, -426],
            [362, -362, -362, 362, 362, -362, -362, 362],
            [284, -502, 100, 426, -426, -100, 502, -284],
            [196, -473, 473, -196, -196, 473, -473, 196],
            [100, -284, 426, -502, 502, -426, 284, -100],
        ];

        COS_TABLE[k][n]
    }

    /// Hadamard transform (used in some codecs)
    pub fn hadamard_4x4(input: &[i16], output: &mut [i16]) {
        let mut temp = [0i32; 16];

        // Horizontal
        for i in 0..4 {
            let a = input[i * 4 + 0] as i32 + input[i * 4 + 2] as i32;
            let b = input[i * 4 + 1] as i32 + input[i * 4 + 3] as i32;
            let c = input[i * 4 + 0] as i32 - input[i * 4 + 2] as i32;
            let d = input[i * 4 + 1] as i32 - input[i * 4 + 3] as i32;

            temp[i * 4 + 0] = a + b;
            temp[i * 4 + 1] = c + d;
            temp[i * 4 + 2] = a - b;
            temp[i * 4 + 3] = c - d;
        }

        // Vertical
        for i in 0..4 {
            let a = temp[0 * 4 + i] + temp[2 * 4 + i];
            let b = temp[1 * 4 + i] + temp[3 * 4 + i];
            let c = temp[0 * 4 + i] - temp[2 * 4 + i];
            let d = temp[1 * 4 + i] - temp[3 * 4 + i];

            output[0 * 4 + i] = ((a + b) >> 1) as i16;
            output[1 * 4 + i] = ((c + d) >> 1) as i16;
            output[2 * 4 + i] = ((a - b) >> 1) as i16;
            output[3 * 4 + i] = ((c - d) >> 1) as i16;
        }
    }
}
