pub struct ChunkLayout {
    pub size: u64,
    pub chunk_size: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Piece {
    pub chunk: u32,
    pub local_start: u64,
    pub local_end_inclusive: u64,
    pub buf_offset: usize,
}

impl ChunkLayout {
    pub fn chunk_count(&self) -> u32 {
        self.size.div_ceil(self.chunk_size) as u32
    }

    pub fn pieces(&self, start: u64, end_inclusive: u64) -> Vec<Piece> {
        if self.size == 0 || start >= self.size {
            return Vec::new();
        }
        let end = end_inclusive.min(self.size - 1);
        let mut pieces = Vec::new();
        let mut pos = start;
        while pos <= end {
            let chunk = pos / self.chunk_size;
            let chunk_end = (chunk + 1) * self.chunk_size - 1;
            let piece_end = end.min(chunk_end);
            pieces.push(Piece {
                chunk: chunk as u32,
                local_start: pos - chunk * self.chunk_size,
                local_end_inclusive: piece_end - chunk * self.chunk_size,
                buf_offset: (pos - start) as usize,
            });
            pos = piece_end + 1;
        }
        pieces
    }

    pub fn chunk_name(index: u32) -> String {
        format!("{index:04}.bin")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_count_rounds_up() {
        assert_eq!(
            ChunkLayout {
                size: 100,
                chunk_size: 40
            }
            .chunk_count(),
            3
        );
        assert_eq!(
            ChunkLayout {
                size: 80,
                chunk_size: 40
            }
            .chunk_count(),
            2
        );
        assert_eq!(
            ChunkLayout {
                size: 0,
                chunk_size: 40
            }
            .chunk_count(),
            0
        );
    }

    #[test]
    fn pieces_split_at_chunk_boundaries_and_clamp_to_size() {
        let layout = ChunkLayout {
            size: 100,
            chunk_size: 40,
        };
        assert_eq!(
            layout.pieces(35, 85),
            vec![
                Piece {
                    chunk: 0,
                    local_start: 35,
                    local_end_inclusive: 39,
                    buf_offset: 0
                },
                Piece {
                    chunk: 1,
                    local_start: 0,
                    local_end_inclusive: 39,
                    buf_offset: 5
                },
                Piece {
                    chunk: 2,
                    local_start: 0,
                    local_end_inclusive: 5,
                    buf_offset: 45
                },
            ]
        );
        assert_eq!(
            layout.pieces(90, 200),
            vec![Piece {
                chunk: 2,
                local_start: 10,
                local_end_inclusive: 19,
                buf_offset: 0
            }]
        );
        assert!(layout.pieces(100, 110).is_empty());
    }

    #[test]
    fn chunk_names_are_zero_padded() {
        assert_eq!(ChunkLayout::chunk_name(0), "0000.bin");
        assert_eq!(ChunkLayout::chunk_name(12), "0012.bin");
    }
}
