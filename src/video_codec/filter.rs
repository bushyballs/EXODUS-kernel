// video_codec/filter.rs - Post-processing filters (deblocking, SAO, CDEF, loop filter)

#![no_std]

use crate::video_codec::types::Frame;

/// H.264 Deblocking filter
pub struct DeblockingFilter;

impl DeblockingFilter {
    /// Apply deblocking filter to frame
    pub fn apply(frame: &mut Frame) {
        let mb_width = (frame.width + 15) / 16;
        let mb_height = (frame.height + 15) / 16;

        // Filter vertical edges
        for mb_y in 0..mb_height {
            for mb_x in 1..mb_width {
                Self::filter_vertical_edge(frame, mb_x * 16, mb_y * 16);
            }
        }

        // Filter horizontal edges
        for mb_y in 1..mb_height {
            for mb_x in 0..mb_width {
                Self::filter_horizontal_edge(frame, mb_x * 16, mb_y * 16);
            }
        }
    }

    fn filter_vertical_edge(frame: &mut Frame, x: u32, y: u32) {
        if x < 4 || x >= frame.width {
            return;
        }

        unsafe {
            for dy in 0..16 {
                if y + dy >= frame.height {
                    break;
                }

                let offset = ((y + dy) * frame.y_stride + x) as isize;

                // Get pixels on both sides of edge
                let p1 = *frame.y_plane.offset(offset - 2) as i32;
                let p0 = *frame.y_plane.offset(offset - 1) as i32;
                let q0 = *frame.y_plane.offset(offset) as i32;
                let q1 = *frame.y_plane.offset(offset + 1) as i32;

                // Calculate delta
                let delta = ((q0 - p0) * 4 + (p1 - q1) + 4) >> 3;
                let delta = delta.clamp(-4, 4);

                // Apply filter
                let new_p0 = (p0 + delta).clamp(0, 255) as u8;
                let new_q0 = (q0 - delta).clamp(0, 255) as u8;

                *frame.y_plane.offset(offset - 1) = new_p0;
                *frame.y_plane.offset(offset) = new_q0;
            }
        }
    }

    fn filter_horizontal_edge(frame: &mut Frame, x: u32, y: u32) {
        if y < 4 || y >= frame.height {
            return;
        }

        unsafe {
            for dx in 0..16 {
                if x + dx >= frame.width {
                    break;
                }

                let offset = ((y) * frame.y_stride + x + dx) as isize;
                let stride = frame.y_stride as isize;

                // Get pixels on both sides of edge
                let p1 = *frame.y_plane.offset(offset - stride * 2) as i32;
                let p0 = *frame.y_plane.offset(offset - stride) as i32;
                let q0 = *frame.y_plane.offset(offset) as i32;
                let q1 = *frame.y_plane.offset(offset + stride) as i32;

                // Calculate delta
                let delta = ((q0 - p0) * 4 + (p1 - q1) + 4) >> 3;
                let delta = delta.clamp(-4, 4);

                // Apply filter
                let new_p0 = (p0 + delta).clamp(0, 255) as u8;
                let new_q0 = (q0 - delta).clamp(0, 255) as u8;

                *frame.y_plane.offset(offset - stride) = new_p0;
                *frame.y_plane.offset(offset) = new_q0;
            }
        }
    }
}

/// H.265 SAO (Sample Adaptive Offset) filter
pub struct SAOFilter;

impl SAOFilter {
    /// Apply SAO filter to frame
    pub fn apply(frame: &mut Frame) {
        let ctu_width = (frame.width + 63) / 64;
        let ctu_height = (frame.height + 63) / 64;

        for ctu_y in 0..ctu_height {
            for ctu_x in 0..ctu_width {
                Self::filter_ctu(frame, ctu_x * 64, ctu_y * 64);
            }
        }
    }

    fn filter_ctu(frame: &mut Frame, x: u32, y: u32) {
        // Simplified SAO - edge offset type
        let width = 64.min(frame.width - x);
        let height = 64.min(frame.height - y);

        unsafe {
            for dy in 1..height - 1 {
                for dx in 1..width - 1 {
                    let offset = ((y + dy) * frame.y_stride + x + dx) as isize;
                    let stride = frame.y_stride as isize;

                    let center = *frame.y_plane.offset(offset) as i32;
                    let left = *frame.y_plane.offset(offset - 1) as i32;
                    let right = *frame.y_plane.offset(offset + 1) as i32;
                    let top = *frame.y_plane.offset(offset - stride) as i32;
                    let bottom = *frame.y_plane.offset(offset + stride) as i32;

                    // Calculate edge category
                    let h_edge = (center - left).signum() + (center - right).signum();
                    let v_edge = (center - top).signum() + (center - bottom).signum();

                    // Apply offset based on category (simplified)
                    let offset_val = match (h_edge, v_edge) {
                        (-2, _) | (_, -2) => -1,
                        (2, _) | (_, 2) => 1,
                        _ => 0,
                    };

                    let new_val = (center + offset_val).clamp(0, 255) as u8;
                    *frame.y_plane.offset(offset) = new_val;
                }
            }
        }
    }
}

/// VP9 Loop filter
pub struct LoopFilter;

impl LoopFilter {
    /// Apply VP9 loop filter
    pub fn apply_vp9(frame: &mut Frame) {
        let sb_width = (frame.width + 63) / 64;
        let sb_height = (frame.height + 63) / 64;

        for sb_y in 0..sb_height {
            for sb_x in 0..sb_width {
                Self::filter_superblock(frame, sb_x * 64, sb_y * 64);
            }
        }
    }

    fn filter_superblock(frame: &mut Frame, x: u32, y: u32) {
        // Filter 8x8 boundaries within the superblock
        for by in (0..64).step_by(8) {
            for bx in (8..64).step_by(8) {
                if x + bx < frame.width && y + by < frame.height {
                    Self::filter_edge_8x8(frame, x + bx, y + by, true);
                }
            }
        }

        for by in (8..64).step_by(8) {
            for bx in (0..64).step_by(8) {
                if x + bx < frame.width && y + by < frame.height {
                    Self::filter_edge_8x8(frame, x + bx, y + by, false);
                }
            }
        }
    }

    fn filter_edge_8x8(frame: &mut Frame, x: u32, y: u32, vertical: bool) {
        let length = 8;

        unsafe {
            for i in 0..length {
                let (px, py) = if vertical {
                    (x, y + i)
                } else {
                    (x + i, y)
                };

                if px < 4 || py < 4 || px >= frame.width || py >= frame.height {
                    continue;
                }

                let offset = (py * frame.y_stride + px) as isize;
                let (p_offset, q_offset) = if vertical {
                    (-1isize, 0isize)
                } else {
                    (-(frame.y_stride as isize), 0isize)
                };

                let p0 = *frame.y_plane.offset(offset + p_offset) as i32;
                let q0 = *frame.y_plane.offset(offset + q_offset) as i32;

                let delta = ((q0 - p0).abs() < 10).then(|| (q0 - p0) / 2).unwrap_or(0);

                let new_p0 = (p0 + delta).clamp(0, 255) as u8;
                let new_q0 = (q0 - delta).clamp(0, 255) as u8;

                *frame.y_plane.offset(offset + p_offset) = new_p0;
                *frame.y_plane.offset(offset + q_offset) = new_q0;
            }
        }
    }
}

/// AV1 CDEF (Constrained Directional Enhancement Filter)
pub struct CDEF;

impl CDEF {
    /// Apply CDEF filter
    pub fn apply(frame: &mut Frame) {
        let sb_width = (frame.width + 63) / 64;
        let sb_height = (frame.height + 63) / 64;

        for sb_y in 0..sb_height {
            for sb_x in 0..sb_width {
                Self::filter_superblock(frame, sb_x * 64, sb_y * 64);
            }
        }
    }

    fn filter_superblock(frame: &mut Frame, x: u32, y: u32) {
        let width = 64.min(frame.width - x);
        let height = 64.min(frame.height - y);

        // Process 8x8 blocks
        for by in (0..height).step_by(8) {
            for bx in (0..width).step_by(8) {
                Self::filter_8x8(frame, x + bx, y + by);
            }
        }
    }

    fn filter_8x8(frame: &mut Frame, x: u32, y: u32) {
        unsafe {
            for dy in 0..8 {
                for dx in 0..8 {
                    if x + dx >= frame.width || y + dy >= frame.height {
                        continue;
                    }

                    let offset = ((y + dy) * frame.y_stride + x + dx) as isize;
                    let pixel = *frame.y_plane.offset(offset) as i32;

                    // Simplified CDEF: directional filter
                    let mut sum = pixel * 4;
                    let mut count = 4;

                    // Add neighboring pixels
                    for (dy_off, dx_off) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                        let nx = (x + dx) as i32 + dx_off;
                        let ny = (y + dy) as i32 + dy_off;

                        if nx >= 0 && ny >= 0 && (nx as u32) < frame.width && (ny as u32) < frame.height {
                            let n_offset = (ny as u32 * frame.y_stride + nx as u32) as isize;
                            let n_pixel = *frame.y_plane.offset(n_offset) as i32;

                            if (n_pixel - pixel).abs() < 16 {
                                sum += n_pixel;
                                count += 1;
                            }
                        }
                    }

                    let filtered = (sum + count / 2) / count;
                    *frame.y_plane.offset(offset) = filtered.clamp(0, 255) as u8;
                }
            }
        }
    }
}
