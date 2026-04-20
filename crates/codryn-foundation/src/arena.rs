/// Bump allocator for batch allocations that share a lifetime.
/// All memory freed at once when the arena is dropped.
pub struct Arena {
    chunks: Vec<Vec<u8>>,
    current: Vec<u8>,
    chunk_size: usize,
}

const DEFAULT_CHUNK: usize = 64 * 1024;

impl Arena {
    pub fn new() -> Self {
        Self::with_chunk_size(DEFAULT_CHUNK)
    }

    pub fn with_chunk_size(size: usize) -> Self {
        Self {
            chunks: Vec::new(),
            current: Vec::with_capacity(size),
            chunk_size: size,
        }
    }

    pub fn alloc_str(&mut self, s: &str) -> &str {
        let bytes = self.alloc_bytes(s.as_bytes());
        unsafe { std::str::from_utf8_unchecked(bytes) }
    }

    pub fn alloc_bytes(&mut self, data: &[u8]) -> &[u8] {
        if self.current.len() + data.len() > self.current.capacity() {
            let old = std::mem::replace(
                &mut self.current,
                Vec::with_capacity(self.chunk_size.max(data.len())),
            );
            if !old.is_empty() {
                self.chunks.push(old);
            }
        }
        let start = self.current.len();
        self.current.extend_from_slice(data);
        // SAFETY: we never mutate previous bytes, and the arena owns the memory
        unsafe {
            let ptr = self.current.as_ptr().add(start);
            std::slice::from_raw_parts(ptr, data.len())
        }
    }

    pub fn bytes_allocated(&self) -> usize {
        self.chunks.iter().map(|c| c.len()).sum::<usize>() + self.current.len()
    }
}

impl Default for Arena {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alloc_str() {
        let mut a = Arena::new();
        a.alloc_str("hello");
        a.alloc_str("world");
    }

    #[test]
    fn test_cross_chunk() {
        let mut a = Arena::with_chunk_size(8);
        a.alloc_str("abcdefgh"); // fills first chunk
        a.alloc_str("ijkl"); // new chunk
        assert!(a.bytes_allocated() >= 12);
    }
}
