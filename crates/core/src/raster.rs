//! Input image: straight (non-premultiplied) RGBA8, row-major, top-left origin.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, RGBA.
    pub data: Vec<u8>,
}

impl Raster {
    pub fn from_rgba8(width: u32, height: u32, data: Vec<u8>) -> Self {
        assert_eq!(data.len(), (width * height * 4) as usize, "RGBA8 size mismatch");
        Self { width, height, data }
    }

    pub fn filled(width: u32, height: u32, rgba: [u8; 4]) -> Self {
        let data = rgba.iter().copied().cycle().take((width * height * 4) as usize).collect();
        Self { width, height, data }
    }

    #[inline]
    pub fn get(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [self.data[i], self.data[i + 1], self.data[i + 2], self.data[i + 3]]
    }

    #[inline]
    pub fn set(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        let i = ((y * self.width + x) * 4) as usize;
        self.data[i..i + 4].copy_from_slice(&rgba);
    }

    /// Most common of the four corner pixels; a crude background guess used by
    /// the M0 stub. M2 replaces this with proper background detection.
    pub fn corner_color(&self) -> [u8; 4] {
        if self.width == 0 || self.height == 0 {
            return [255, 255, 255, 255];
        }
        let (w, h) = (self.width - 1, self.height - 1);
        let cs = [self.get(0, 0), self.get(w, 0), self.get(0, h), self.get(w, h)];
        let mut best = cs[0];
        let mut best_n = 0;
        for &c in &cs {
            let n = cs.iter().filter(|&&d| d == c).count();
            if n > best_n {
                best_n = n;
                best = c;
            }
        }
        best
    }
}
