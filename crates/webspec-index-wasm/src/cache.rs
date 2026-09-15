use std::collections::{HashMap, VecDeque};

use crate::layout::ChunkLayout;
use crate::source::{RangeSource, SourceError};

pub const BLOCK_SIZE: u64 = 64 * 1024;
pub const CACHE_CAP_BYTES: u64 = 64 * 1024 * 1024;

pub struct BlockCache {
    blocks: HashMap<u64, Vec<u8>>,
    order: VecDeque<u64>,
    bytes: u64,
    cap: u64,
}

impl BlockCache {
    pub fn new(cap_bytes: u64) -> Self {
        Self {
            blocks: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            cap: cap_bytes,
        }
    }

    pub fn get(&mut self, block: u64) -> Option<&[u8]> {
        if self.blocks.contains_key(&block) {
            self.order.retain(|&b| b != block);
            self.order.push_back(block);
            Some(self.blocks[&block].as_slice())
        } else {
            None
        }
    }

    pub fn insert(&mut self, block: u64, data: Vec<u8>) {
        self.bytes += data.len() as u64;
        self.order.push_back(block);
        self.blocks.insert(block, data);
        while self.bytes > self.cap {
            if let Some(oldest) = self.order.pop_front() {
                if let Some(evicted) = self.blocks.remove(&oldest) {
                    self.bytes -= evicted.len() as u64;
                }
            } else {
                break;
            }
        }
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

#[derive(Default, Clone, serde::Serialize)]
pub struct Stats {
    pub fetches: u64,
    pub bytes_fetched: u64,
    pub cache_hits: u64,
}

pub fn read_through(
    layout: &ChunkLayout,
    cache: &mut BlockCache,
    stats: &mut Stats,
    source: &dyn RangeSource,
    buf: &mut [u8],
    offset: u64,
) -> Result<bool, SourceError> {
    let mut done = 0usize;
    while done < buf.len() {
        let pos = offset + done as u64;
        if pos >= layout.size {
            buf[done..].fill(0);
            return Ok(false);
        }
        let block = pos / BLOCK_SIZE;
        if cache.get(block).is_none() {
            let start = block * BLOCK_SIZE;
            let end = (start + BLOCK_SIZE - 1).min(layout.size - 1);
            let mut data = vec![0u8; (end - start + 1) as usize];
            for piece in layout.pieces(start, end) {
                let bytes =
                    source.fetch(piece.chunk, piece.local_start, piece.local_end_inclusive)?;
                stats.fetches += 1;
                stats.bytes_fetched += bytes.len() as u64;
                data[piece.buf_offset..piece.buf_offset + bytes.len()].copy_from_slice(&bytes);
            }
            cache.insert(block, data);
        } else {
            stats.cache_hits += 1;
        }
        let data = cache.get(block).expect("just inserted");
        let local = (pos - block * BLOCK_SIZE) as usize;
        let n = (buf.len() - done).min(data.len() - local);
        buf[done..done + n].copy_from_slice(&data[local..local + n]);
        done += n;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::ChunkLayout;
    use crate::source::RangeSource;
    use std::cell::RefCell;

    struct MemSource {
        data: Vec<u8>,
        chunk_size: u64,
        calls: RefCell<Vec<(u32, u64, u64)>>,
    }

    impl RangeSource for MemSource {
        fn fetch(&self, chunk: u32, start: u64, end: u64) -> Result<Vec<u8>, SourceError> {
            self.calls.borrow_mut().push((chunk, start, end));
            let base = chunk as u64 * self.chunk_size;
            Ok(self.data[(base + start) as usize..=(base + end) as usize].to_vec())
        }
    }

    fn source(len: usize, chunk_size: u64) -> MemSource {
        MemSource {
            data: (0..len).map(|i| (i % 251) as u8).collect(),
            chunk_size,
            calls: RefCell::new(Vec::new()),
        }
    }

    // Block 1 = bytes 65536..131071 (BLOCK_SIZE = 64 KiB).
    // With chunk_size 100_000: chunk 0 ends at 99999, chunk 1 starts at 100000.
    // Block 1 spans both chunks → 2 fetches. (With chunk_size 131072 block 1
    // fits entirely in chunk 0 → only 1 fetch, inconsistent with the assertion.)
    #[test]
    fn read_through_fetches_whole_blocks_once_and_serves_repeats_from_cache() {
        let src = source(300 * 1024, 100_000);
        let layout = ChunkLayout {
            size: 300 * 1024,
            chunk_size: 100_000,
        };
        let mut cache = BlockCache::new(CACHE_CAP_BYTES);
        let mut stats = Stats::default();
        let mut buf = vec![0u8; 100];
        assert!(read_through(&layout, &mut cache, &mut stats, &src, &mut buf, 70_000).unwrap());
        assert_eq!(buf, src.data[70_000..70_100]);
        assert_eq!(
            stats.fetches,
            2,
            "block 1 spans two chunks: {:?}",
            src.calls.borrow()
        );
        assert!(read_through(&layout, &mut cache, &mut stats, &src, &mut buf, 70_050).unwrap());
        assert_eq!(stats.fetches, 2);
        assert_eq!(stats.cache_hits, 1);
    }

    #[test]
    fn read_past_end_is_short_and_zero_filled() {
        let src = source(1000, 1000);
        let layout = ChunkLayout {
            size: 1000,
            chunk_size: 1000,
        };
        let mut cache = BlockCache::new(CACHE_CAP_BYTES);
        let mut stats = Stats::default();
        let mut buf = vec![7u8; 16];
        assert!(!read_through(&layout, &mut cache, &mut stats, &src, &mut buf, 992).unwrap());
        assert_eq!(&buf[..8], &src.data[992..1000]);
        assert!(buf[8..].iter().all(|b| *b == 0));
    }

    #[test]
    fn cache_evicts_least_recently_used_when_over_cap() {
        let mut cache = BlockCache::new(3 * BLOCK_SIZE);
        for block in 0..3 {
            cache.insert(block, vec![0; BLOCK_SIZE as usize]);
        }
        cache.get(0);
        cache.insert(3, vec![0; BLOCK_SIZE as usize]);
        assert!(cache.get(1).is_none(), "block 1 was least recently used");
        assert!(cache.get(0).is_some());
        assert_eq!(cache.len(), 3);
        assert!(cache.bytes() <= 3 * BLOCK_SIZE);
    }
}
