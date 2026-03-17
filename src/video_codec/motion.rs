// video_codec/motion.rs - Motion compensation

#![no_std]

use crate::video_codec::types::{Frame, MotionVector};

/// Motion compensation operations
pub struct MotionCompensation;

impl MotionCompensation {
    /// Compensate 16x16 macroblock (H.264)
    pub fn compensate_16x16(reference: &Frame, output: &mut Frame, mb_x: u32, mb_y: u32, mv: MotionVector) {
        let ref_x = (mb_x * 16) as i32 + mv.x as i32;
        let ref_y = (mb_y * 16) as i32 + mv.y as i32;

        Self::copy_block(reference, output, ref_x, ref_y, mb_x * 16, mb_y * 16, 16, 16);
    }

    /// Compensate arbitrary block size (H.265, VP9, AV1)
    pub fn compensate(reference: &Frame, output: &mut Frame, x: u32, y: u32, size: u32, mv: MotionVector) {
        let ref_x = x as i32 + mv.x as i32;
        let ref_y = y as i32 + mv.y as i32;

        Self::copy_block(reference, output, ref_x, ref_y, x, y, size, size);
    }

    /// Copy block with bounds checking
    fn copy_block(
        reference: &Frame,
        output: &mut Frame,
        src_x: i32,
        src_y: i32,
        dst_x: u32,
        dst_y: u32,
        width: u32,
        height: u32,
    ) {
        unsafe {
            for dy in 0..height {
                for dx in 0..width {
                    let ref_px = src_x + dx as i32;
                    let ref_py = src_y + dy as i32;

                    // Bounds check for reference frame
                    if ref_px >= 0 && ref_py >= 0
                        && (ref_px as u32) < reference.width
                        && (ref_py as u32) < reference.height {

                        let src_offset = (ref_py as u32 * reference.y_stride + ref_px as u32) as isize;
                        let dst_offset = ((dst_y + dy) * output.y_stride + dst_x + dx) as isize;

                        if dst_x + dx < output.width && dst_y + dy < output.height {
                            let pixel = *reference.y_plane.offset(src_offset);
                            *output.y_plane.offset(dst_offset) = pixel;
                        }
                    }
                }
            }

            // Also copy chroma planes (simplified: copy at half resolution)
            let chroma_width = width / 2;
            let chroma_height = height / 2;

            for dy in 0..chroma_height {
                for dx in 0..chroma_width {
                    let ref_px = (src_x / 2) + dx as i32;
                    let ref_py = (src_y / 2) + dy as i32;

                    if ref_px >= 0 && ref_py >= 0
                        && (ref_px as u32) < reference.width / 2
                        && (ref_py as u32) < reference.height / 2 {

                        let src_offset = (ref_py as u32 * reference.u_stride + ref_px as u32) as isize;
                        let dst_offset = ((dst_y / 2 + dy) * output.u_stride + dst_x / 2 + dx) as isize;

                        if dst_x / 2 + dx < output.width / 2 && dst_y / 2 + dy < output.height / 2 {
                            // U plane
                            let pixel_u = *reference.u_plane.offset(src_offset);
                            *output.u_plane.offset(dst_offset) = pixel_u;

                            // V plane
                            let pixel_v = *reference.v_plane.offset(src_offset);
                            *output.v_plane.offset(dst_offset) = pixel_v;
                        }
                    }
                }
            }
        }
    }

    /// Sub-pixel motion compensation (quarter-pixel precision)
    pub fn compensate_subpel(
        reference: &Frame,
        output: &mut Frame,
        x: u32,
        y: u32,
        size: u32,
        mv: MotionVector,
    ) {
        // Motion vector in quarter-pixel precision
        let ref_x_int = x as i32 + (mv.x >> 2) as i32;
        let ref_y_int = y as i32 + (mv.y >> 2) as i32;
        let frac_x = (mv.x & 0x3) as u32;
        let frac_y = (mv.y & 0x3) as u32;

        if frac_x == 0 && frac_y == 0 {
            // Integer pixel
            Self::copy_block(reference, output, ref_x_int, ref_y_int, x, y, size, size);
        } else if frac_y == 0 {
            // Horizontal interpolation
            Self::interpolate_horizontal(reference, output, ref_x_int, ref_y_int, x, y, size, frac_x);
        } else if frac_x == 0 {
            // Vertical interpolation
            Self::interpolate_vertical(reference, output, ref_x_int, ref_y_int, x, y, size, frac_y);
        } else {
            // 2D interpolation
            Self::interpolate_2d(reference, output, ref_x_int, ref_y_int, x, y, size, frac_x, frac_y);
        }
    }

    /// Horizontal interpolation
    fn interpolate_horizontal(
        reference: &Frame,
        output: &mut Frame,
        src_x: i32,
        src_y: i32,
        dst_x: u32,
        dst_y: u32,
        size: u32,
        frac: u32,
    ) {
        // 6-tap interpolation filter
        const FILTERS: [[i32; 6]; 4] = [
            [0, 0, 64, 0, 0, 0],      // 0/4
            [-1, 3, 58, 17, -4, 1],   // 1/4
            [-2, 5, 46, 28, -6, 2],   // 2/4
            [-1, 4, 36, 36, -7, 2],   // 3/4
        ];

        let filter = &FILTERS[frac as usize];

        unsafe {
            for dy in 0..size {
                for dx in 0..size {
                    let mut sum = 0i32;

                    for i in 0..6 {
                        let ref_px = src_x + dx as i32 + i - 2;
                        let ref_py = src_y + dy as i32;

                        if ref_px >= 0 && ref_py >= 0
                            && (ref_px as u32) < reference.width
                            && (ref_py as u32) < reference.height {

                            let offset = (ref_py as u32 * reference.y_stride + ref_px as u32) as isize;
                            let pixel = *reference.y_plane.offset(offset) as i32;
                            sum += pixel * filter[i as usize];
                        }
                    }

                    let dst_offset = ((dst_y + dy) * output.y_stride + dst_x + dx) as isize;
                    if dst_x + dx < output.width && dst_y + dy < output.height {
                        *output.y_plane.offset(dst_offset) = ((sum + 32) >> 6).clamp(0, 255) as u8;
                    }
                }
            }
        }
    }

    /// Vertical interpolation
    fn interpolate_vertical(
        reference: &Frame,
        output: &mut Frame,
        src_x: i32,
        src_y: i32,
        dst_x: u32,
        dst_y: u32,
        size: u32,
        frac: u32,
    ) {
        const FILTERS: [[i32; 6]; 4] = [
            [0, 0, 64, 0, 0, 0],
            [-1, 3, 58, 17, -4, 1],
            [-2, 5, 46, 28, -6, 2],
            [-1, 4, 36, 36, -7, 2],
        ];

        let filter = &FILTERS[frac as usize];

        unsafe {
            for dy in 0..size {
                for dx in 0..size {
                    let mut sum = 0i32;

                    for i in 0..6 {
                        let ref_px = src_x + dx as i32;
                        let ref_py = src_y + dy as i32 + i - 2;

                        if ref_px >= 0 && ref_py >= 0
                            && (ref_px as u32) < reference.width
                            && (ref_py as u32) < reference.height {

                            let offset = (ref_py as u32 * reference.y_stride + ref_px as u32) as isize;
                            let pixel = *reference.y_plane.offset(offset) as i32;
                            sum += pixel * filter[i as usize];
                        }
                    }

                    let dst_offset = ((dst_y + dy) * output.y_stride + dst_x + dx) as isize;
                    if dst_x + dx < output.width && dst_y + dy < output.height {
                        *output.y_plane.offset(dst_offset) = ((sum + 32) >> 6).clamp(0, 255) as u8;
                    }
                }
            }
        }
    }

    /// 2D interpolation
    fn interpolate_2d(
        reference: &Frame,
        output: &mut Frame,
        src_x: i32,
        src_y: i32,
        dst_x: u32,
        dst_y: u32,
        size: u32,
        frac_x: u32,
        frac_y: u32,
    ) {
        // Simplified: average of horizontal and vertical interpolation
        let mut temp_h = [0u8; 64 * 64];
        let mut temp_v = [0u8; 64 * 64];

        // Would normally do proper 2D filtering here
        // For simplicity, just copy the integer position
        Self::copy_block(reference, output, src_x, src_y, dst_x, dst_y, size, size);
    }
}
