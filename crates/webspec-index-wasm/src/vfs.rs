use std::cell::RefCell;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::time::Duration;

use sqlite_wasm_rs::utils::ffi;
use sqlite_wasm_rs::utils::{
    register_vfs, OsCallback, SQLiteIoMethods, SQLiteVfs, SQLiteVfsFile, VfsAppData, VfsError,
    VfsFile, VfsResult, VfsStore,
};
use sqlite_wasm_rs::WasmOsCallback;

use crate::cache::{read_through, BlockCache, Stats};
use crate::layout::ChunkLayout;
use crate::source::{RangeSource, SourceError};

pub struct HttpFile {
    layout: ChunkLayout,
    source: Box<dyn RangeSource>,
    cache: RefCell<BlockCache>,
    stats: RefCell<Stats>,
}

impl HttpFile {
    pub fn new(layout: ChunkLayout, source: Box<dyn RangeSource>, cap_bytes: u64) -> Self {
        Self {
            layout,
            source,
            cache: RefCell::new(BlockCache::new(cap_bytes)),
            stats: RefCell::new(Stats::default()),
        }
    }
}

impl VfsFile for HttpFile {
    fn read(&self, buf: &mut [u8], offset: usize) -> VfsResult<bool> {
        read_through(
            &self.layout,
            &mut self.cache.borrow_mut(),
            &mut self.stats.borrow_mut(),
            self.source.as_ref(),
            buf,
            offset as u64,
        )
        .map_err(|SourceError(msg)| VfsError::new(ffi::SQLITE_IOERR_READ, msg))
    }

    fn write(&mut self, _buf: &[u8], _offset: usize) -> VfsResult<()> {
        Err(VfsError::new(
            ffi::SQLITE_READONLY,
            "http vfs is read-only".to_string(),
        ))
    }

    fn truncate(&mut self, _size: usize) -> VfsResult<()> {
        Err(VfsError::new(
            ffi::SQLITE_READONLY,
            "http vfs is read-only".to_string(),
        ))
    }

    fn flush(&mut self) -> VfsResult<()> {
        Ok(())
    }

    fn size(&self) -> VfsResult<usize> {
        Ok(self.layout.size as usize)
    }
}

type AppData = RefCell<HashMap<String, HttpFile>>;

struct HttpStore;

impl VfsStore<HttpFile, AppData> for HttpStore {
    fn add_file(_vfs: *mut ffi::sqlite3_vfs, file: &str, _flags: i32) -> VfsResult<()> {
        Err(VfsError::new(
            ffi::SQLITE_CANTOPEN,
            format!("{file}: files must be registered up front"),
        ))
    }

    fn contains_file(vfs: *mut ffi::sqlite3_vfs, file: &str) -> VfsResult<bool> {
        let app_data = unsafe { Self::app_data(vfs) };
        Ok(app_data.borrow().contains_key(file))
    }

    fn delete_file(_vfs: *mut ffi::sqlite3_vfs, _file: &str) -> VfsResult<()> {
        Ok(())
    }

    fn with_file<F: Fn(&HttpFile) -> VfsResult<i32>>(
        vfs_file: &SQLiteVfsFile,
        f: F,
    ) -> VfsResult<i32> {
        let name = unsafe { vfs_file.name() };
        let app_data = unsafe { Self::app_data(vfs_file.vfs) };
        match app_data.borrow().get(name) {
            Some(file) => f(file),
            None => Err(VfsError::new(
                ffi::SQLITE_IOERR,
                format!("{name} not found"),
            )),
        }
    }

    fn with_file_mut<F: Fn(&mut HttpFile) -> VfsResult<i32>>(
        vfs_file: &SQLiteVfsFile,
        f: F,
    ) -> VfsResult<i32> {
        let name = unsafe { vfs_file.name() };
        let app_data = unsafe { Self::app_data(vfs_file.vfs) };
        match app_data.borrow_mut().get_mut(name) {
            Some(file) => f(file),
            None => Err(VfsError::new(
                ffi::SQLITE_IOERR,
                format!("{name} not found"),
            )),
        }
    }
}

struct HttpIoMethods;

impl SQLiteIoMethods for HttpIoMethods {
    type File = HttpFile;
    type AppData = AppData;
    type Store = HttpStore;
    const VERSION: i32 = 1;
}

struct HttpVfs<C>(PhantomData<C>);

impl<C: OsCallback> SQLiteVfs<HttpIoMethods> for HttpVfs<C> {
    const VERSION: i32 = 1;

    fn sleep(dur: Duration) {
        C::sleep(dur)
    }

    fn random(buf: &mut [u8]) {
        C::random(buf)
    }

    fn epoch_timestamp_in_ms() -> i64 {
        C::epoch_timestamp_in_ms()
    }
}

thread_local! {
    static APP_DATA: RefCell<Option<&'static VfsAppData<AppData>>> =
        const { RefCell::new(None) };
}

fn get_app_data() -> &'static VfsAppData<AppData> {
    APP_DATA.with(|slot| {
        *slot.borrow_mut().get_or_insert_with(|| {
            let vfs = register_vfs::<HttpIoMethods, HttpVfs<WasmOsCallback>>(
                "http",
                RefCell::new(HashMap::new()),
                false,
            )
            .expect("register http vfs");
            unsafe { HttpStore::app_data(vfs) }
        })
    })
}

pub fn register_file(name: &str, file: HttpFile) -> Result<(), String> {
    get_app_data().borrow_mut().insert(name.to_string(), file);
    Ok(())
}

pub fn stats(name: &str) -> Option<Stats> {
    get_app_data()
        .borrow()
        .get(name)
        .map(|f| f.stats.borrow().clone())
}
