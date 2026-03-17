fn clamp_i8(value: i32) -> i8 {
    if value > i8::MAX as i32 {
        i8::MAX
    } else if value < i8::MIN as i32 {
        i8::MIN
    } else {
        value as i8
    }
}

fn saturating_mul_i32(a: i32, b: i32) -> i32 {
    let prod = (a as i64) * (b as i64);
    if prod > i32::MAX as i64 {
        i32::MAX
    } else if prod < i32::MIN as i64 {
        i32::MIN
    } else {
        prod as i32
    }
}

fn saturating_shl_i32(value: i32, shift: u32) -> i32 {
    if shift >= 31 {
        if value >= 0 {
            i32::MAX
        } else {
            i32::MIN
        }
    } else {
        let widened = (value as i64) << shift;
        if widened > i32::MAX as i64 {
            i32::MAX
        } else if widened < i32::MIN as i64 {
            i32::MIN
        } else {
            widened as i32
        }
    }
}

fn requantize(acc: i32, multiplier: i32, shift: i32, zero_point: i32) -> i8 {
    let scaled = saturating_mul_i32(acc, multiplier);
    let shifted = if shift >= 0 {
        let right = if shift > 30 { 30 } else { shift as u32 };
        if right == 0 {
            scaled
        } else {
            let rounding = 1i32 << (right - 1);
            scaled.saturating_add(rounding) >> right
        }
    } else {
        let left = (-shift) as u32;
        saturating_shl_i32(scaled, left)
    };
    let with_zero = shifted.saturating_add(zero_point);
    clamp_i8(with_zero)
}

pub fn matmul_int8(
    a: &[i8],
    b: &[i8],
    c: &mut [i8],
    m: usize,
    k: usize,
    n: usize,
    a_zero_point: i32,
    b_zero_point: i32,
    c_zero_point: i32,
    multiplier: i32,
    shift: i32,
) {
    if m == 0 || k == 0 || n == 0 {
        return;
    }

    let a_needed = match m.checked_mul(k) {
        Some(v) => v,
        None => return,
    };
    let b_needed = match k.checked_mul(n) {
        Some(v) => v,
        None => return,
    };
    let c_needed = match m.checked_mul(n) {
        Some(v) => v,
        None => return,
    };

    if a.len() < a_needed || b.len() < b_needed || c.len() < c_needed {
        return;
    }

    let mut row = 0usize;
    while row < m {
        let mut col = 0usize;
        while col < n {
            let mut acc = 0i32;
            let mut inner = 0usize;
            while inner < k {
                let a_idx = row * k + inner;
                let b_idx = inner * n + col;
                let a_val = (a[a_idx] as i32).saturating_sub(a_zero_point);
                let b_val = (b[b_idx] as i32).saturating_sub(b_zero_point);
                let prod = saturating_mul_i32(a_val, b_val);
                acc = acc.saturating_add(prod);
                inner = inner.saturating_add(1);
            }

            let c_idx = row * n + col;
            c[c_idx] = requantize(acc, multiplier, shift, c_zero_point);
            col = col.saturating_add(1);
        }
        row = row.saturating_add(1);
    }
}

pub fn relu_i8(values: &mut [i8]) {
    let mut i = 0usize;
    while i < values.len() {
        if values[i] < 0 {
            values[i] = 0;
        }
        i = i.saturating_add(1);
    }
}

fn exp_shift_approx(diff: i32) -> u32 {
    if diff <= 0 {
        return 256;
    }
    let clamped = if diff > 15 { 15 } else { diff as u32 };
    let shifted = 256u32 >> clamped;
    if shifted == 0 {
        1
    } else {
        shifted
    }
}

pub fn softmax_fixed(input: &[i8], output: &mut [u16], len: usize) {
    if len == 0 || input.len() < len || output.len() < len {
        return;
    }

    let mut i = 0usize;
    let mut max_val = input[0];
    while i < len {
        if input[i] > max_val {
            max_val = input[i];
        }
        i = i.saturating_add(1);
    }

    let mut sum = 0u32;
    i = 0;
    while i < len {
        let diff = (max_val as i32).saturating_sub(input[i] as i32);
        let e = exp_shift_approx(diff);
        sum = sum.saturating_add(e);
        i = i.saturating_add(1);
    }

    if sum == 0 {
        let mut z = 0usize;
        while z < len {
            output[z] = 0;
            z = z.saturating_add(1);
        }
        return;
    }

    i = 0;
    while i < len {
        let diff = (max_val as i32).saturating_sub(input[i] as i32);
        let e = exp_shift_approx(diff);
        let numerator = e.saturating_mul(65535u32);
        let prob = numerator / sum;
        output[i] = if prob > u16::MAX as u32 {
            u16::MAX
        } else {
            prob as u16
        };
        i = i.saturating_add(1);
    }
}
