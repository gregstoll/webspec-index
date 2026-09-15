#[cfg(not(target_arch = "wasm32"))]
use crate::layout::ChunkLayout;

pub trait RangeSource {
    fn fetch(
        &self,
        chunk: u32,
        local_start: u64,
        local_end_inclusive: u64,
    ) -> Result<Vec<u8>, SourceError>;
}

#[derive(Debug)]
pub struct SourceError(pub String);

#[cfg(not(target_arch = "wasm32"))]
pub struct DirSource {
    pub dir: std::path::PathBuf,
}

#[cfg(not(target_arch = "wasm32"))]
impl RangeSource for DirSource {
    fn fetch(
        &self,
        chunk: u32,
        local_start: u64,
        local_end_inclusive: u64,
    ) -> Result<Vec<u8>, SourceError> {
        use std::io::{Read, Seek, SeekFrom};
        let path = self.dir.join(ChunkLayout::chunk_name(chunk));
        let mut file = std::fs::File::open(&path)
            .map_err(|e| SourceError(format!("{}: {e}", path.display())))?;
        file.seek(SeekFrom::Start(local_start))
            .map_err(|e| SourceError(format!("{}: seek: {e}", path.display())))?;
        let count = (local_end_inclusive - local_start + 1) as usize;
        let mut buf = vec![0u8; count];
        file.read_exact(&mut buf)
            .map_err(|e| SourceError(format!("{}: read: {e}", path.display())))?;
        Ok(buf)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn dir_source_reads_ranges_from_chunk_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("0000.bin"), (0u8..100).collect::<Vec<_>>()).unwrap();
        std::fs::write(
            dir.path().join("0001.bin"),
            (100u8..200).collect::<Vec<_>>(),
        )
        .unwrap();
        let src = DirSource {
            dir: dir.path().to_path_buf(),
        };
        assert_eq!(src.fetch(0, 10, 12).unwrap(), vec![10, 11, 12]);
        assert_eq!(src.fetch(1, 0, 1).unwrap(), vec![100, 101]);
        assert!(src.fetch(2, 0, 1).is_err());
    }
}
