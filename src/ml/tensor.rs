#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TensorDtype {
    Int8 = 0,
    Int16 = 1,
    Int32 = 2,
    Q8_8 = 3,
    Q16_16 = 4,
    Binary = 5,
}

impl TensorDtype {
    pub const fn empty() -> Self {
        TensorDtype::Int8
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TensorShape {
    pub dims: [usize; 4],
    pub ndim: u8,
}

impl TensorShape {
    pub const fn empty() -> Self {
        Self {
            dims: [0; 4],
            ndim: 0,
        }
    }

    pub const fn from_1d(size: usize) -> Self {
        Self {
            dims: [size, 0, 0, 0],
            ndim: 1,
        }
    }

    pub fn element_count(&self) -> usize {
        if self.ndim == 0 {
            return 0;
        }
        let mut i = 0usize;
        let mut total = 1usize;
        while i < self.ndim as usize && i < 4 {
            total = total.saturating_mul(self.dims[i]);
            i = i.saturating_add(1);
        }
        total
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TensorSmall {
    pub data: [i32; 4096],
    pub shape: TensorShape,
    pub dtype: TensorDtype,
    pub scale: i32,
    pub zero_pt: i32,
}

impl TensorSmall {
    pub const fn empty() -> Self {
        Self {
            data: [0; 4096],
            shape: TensorShape::empty(),
            dtype: TensorDtype::Int32,
            scale: 1,
            zero_pt: 0,
        }
    }

    pub const fn zeros() -> Self {
        Self {
            data: [0; 4096],
            shape: TensorShape::empty(),
            dtype: TensorDtype::Int32,
            scale: 1,
            zero_pt: 0,
        }
    }
}
