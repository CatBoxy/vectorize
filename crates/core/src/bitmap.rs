//! One byte per pixel binary image. Out-of-range reads are `false`, which is
//! what the path walker wants at the image border.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitmap {
    pub width: usize,
    pub height: usize,
    pub bits: Vec<u8>,
}

impl Bitmap {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height, bits: vec![0; width * height] }
    }

    #[inline]
    pub fn get(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            return false;
        }
        self.bits[y as usize * self.width + x as usize] != 0
    }

    #[inline]
    pub fn set(&mut self, x: usize, y: usize, v: bool) {
        self.bits[y * self.width + x] = v as u8;
    }

    #[inline]
    pub fn toggle(&mut self, x: i32, y: i32) {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            return;
        }
        let i = y as usize * self.width + x as usize;
        self.bits[i] ^= 1;
    }

    /// Index of the first set pixel at or after `from` in row-major order.
    pub fn next_set(&self, from: usize) -> Option<usize> {
        self.bits.get(from..)?.iter().position(|&b| b != 0).map(|p| p + from)
    }

    pub fn count(&self) -> usize {
        self.bits.iter().filter(|&&b| b != 0).count()
    }
}
