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
    last_fetched: Option<u64>,
    streak: u32,
}

impl BlockCache {
    pub fn new(cap_bytes: u64) -> Self {
        Self {
            blocks: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            cap: cap_bytes,
            last_fetched: None,
            streak: 0,
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
        let local = (pos - block * BLOCK_SIZE) as usize;
        let n = if let Some(data) = cache.get(block) {
            stats.cache_hits += 1;
            copy_block(buf, done, data, local)
        } else {
            let span = if cache.last_fetched == Some(block.wrapping_sub(1)) {
                cache.streak = (cache.streak + 1).min(4);
                1u64 << cache.streak
            } else {
                cache.streak = 0;
                1
            };
            let start = block * BLOCK_SIZE;
            let end = (start + span * BLOCK_SIZE - 1).min(layout.size - 1);
            let mut data = vec![0u8; (end - start + 1) as usize];
            for piece in layout.pieces(start, end) {
                let bytes =
                    source.fetch(piece.chunk, piece.local_start, piece.local_end_inclusive)?;
                stats.fetches += 1;
                stats.bytes_fetched += bytes.len() as u64;
                data[piece.buf_offset..piece.buf_offset + bytes.len()].copy_from_slice(&bytes);
            }
            let n = copy_block(
                buf,
                done,
                &data[..BLOCK_SIZE.min(data.len() as u64) as usize],
                local,
            );
            for (i, chunk) in data.chunks(BLOCK_SIZE as usize).enumerate() {
                cache.insert(block + i as u64, chunk.to_vec());
            }
            cache.last_fetched = Some(end / BLOCK_SIZE);
            n
        };
        done += n;
    }
    Ok(true)
}

fn copy_block(buf: &mut [u8], done: usize, data: &[u8], local: usize) -> usize {
    let n = (buf.len() - done).min(data.len() - local);
    buf[done..done + n].copy_from_slice(&data[local..local + n]);
    n
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
    fn read_through_serves_data_even_when_cache_cannot_hold_one_block() {
        let src = source(200 * 1024, 100_000);
        let layout = ChunkLayout {
            size: 200 * 1024,
            chunk_size: 100_000,
        };
        let mut cache = BlockCache::new(1);
        let mut stats = Stats::default();
        let mut buf = vec![0u8; 100];
        assert!(read_through(&layout, &mut cache, &mut stats, &src, &mut buf, 65_500).unwrap());
        assert_eq!(buf, src.data[65_500..65_600]);
        assert!(read_through(&layout, &mut cache, &mut stats, &src, &mut buf, 65_500).unwrap());
        assert_eq!(stats.cache_hits, 0);
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
    fn sequential_misses_grow_the_fetch_span_up_to_sixteen_blocks() {
        let src = source((64 * BLOCK_SIZE) as usize, 1 << 30);
        let layout = ChunkLayout {
            size: 64 * BLOCK_SIZE,
            chunk_size: 1 << 30,
        };
        let mut cache = BlockCache::new(CACHE_CAP_BYTES);
        let mut stats = Stats::default();
        let mut buf = vec![0u8; BLOCK_SIZE as usize];
        for block in 0..40u64 {
            read_through(
                &layout,
                &mut cache,
                &mut stats,
                &src,
                &mut buf,
                block * BLOCK_SIZE,
            )
            .unwrap();
            assert_eq!(buf[0], ((block * BLOCK_SIZE) % 251) as u8);
        }
        // 1 + 2 + 4 + 8 + 16 + 16 = 47 blocks covered by 6 fetches; block 40 not needed.
        let calls = src.calls.borrow();
        assert_eq!(calls.len(), 6, "{calls:?}");
        let spans: Vec<u64> = calls
            .iter()
            .map(|(_, s, e)| (e - s + 1) / BLOCK_SIZE)
            .collect();
        assert_eq!(spans, vec![1, 2, 4, 8, 16, 16]);
    }

    #[test]
    fn random_miss_resets_the_span_to_one_block() {
        let src = source((64 * BLOCK_SIZE) as usize, 1 << 30);
        let layout = ChunkLayout {
            size: 64 * BLOCK_SIZE,
            chunk_size: 1 << 30,
        };
        let mut cache = BlockCache::new(CACHE_CAP_BYTES);
        let mut stats = Stats::default();
        let mut buf = vec![0u8; 16];
        read_through(&layout, &mut cache, &mut stats, &src, &mut buf, 0).unwrap();
        read_through(&layout, &mut cache, &mut stats, &src, &mut buf, BLOCK_SIZE).unwrap();
        read_through(
            &layout,
            &mut cache,
            &mut stats,
            &src,
            &mut buf,
            50 * BLOCK_SIZE,
        )
        .unwrap();
        let calls = src.calls.borrow();
        let spans: Vec<u64> = calls
            .iter()
            .map(|(_, s, e)| (e - s + 1) / BLOCK_SIZE)
            .collect();
        assert_eq!(spans, vec![1, 2, 1]);
    }

    #[test]
    fn read_ahead_stops_at_end_of_file() {
        let size = 3 * BLOCK_SIZE + 100;
        let src = source(size as usize, 1 << 30);
        let layout = ChunkLayout {
            size,
            chunk_size: 1 << 30,
        };
        let mut cache = BlockCache::new(CACHE_CAP_BYTES);
        let mut stats = Stats::default();
        let mut buf = vec![0u8; 16];
        read_through(&layout, &mut cache, &mut stats, &src, &mut buf, 0).unwrap();
        read_through(&layout, &mut cache, &mut stats, &src, &mut buf, BLOCK_SIZE).unwrap();
        read_through(
            &layout,
            &mut cache,
            &mut stats,
            &src,
            &mut buf,
            3 * BLOCK_SIZE,
        )
        .unwrap();
        assert_eq!(stats.bytes_fetched, size);
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
