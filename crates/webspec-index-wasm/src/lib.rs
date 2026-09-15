pub mod cache;
pub mod layout;
pub mod source;
#[cfg(target_arch = "wasm32")]
pub mod vfs;
#[cfg(target_arch = "wasm32")]
pub mod xhr;

#[cfg(target_arch = "wasm32")]
mod exports {
    use std::cell::RefCell;

    use rusqlite::{Connection, OpenFlags};
    use wasm_bindgen::prelude::*;

    use crate::{cache, layout, vfs, xhr};

    thread_local! {
        static CONN: RefCell<Option<Connection>> = const { RefCell::new(None) };
    }

    #[derive(serde::Deserialize)]
    struct ManifestHeader {
        size: u64,
        chunk_size: u64,
    }

    #[wasm_bindgen]
    pub fn open(manifest_url: &str) -> Result<(), JsValue> {
        let base_url = manifest_url
            .rsplit_once('/')
            .map(|(base, _)| base.to_string())
            .unwrap_or_default();
        let manifest: ManifestHeader = serde_json::from_str(
            &xhr::fetch_text_sync(manifest_url).map_err(|e| JsValue::from_str(&e.0))?,
        )
        .map_err(|e| JsValue::from_str(&format!("manifest.json: {e}")))?;
        let file = vfs::HttpFile::new(
            layout::ChunkLayout {
                size: manifest.size,
                chunk_size: manifest.chunk_size,
            },
            Box::new(xhr::XhrSource { base_url }),
            cache::CACHE_CAP_BYTES,
        );
        vfs::register_file("webspec.db", file).map_err(|e| JsValue::from_str(&e))?;
        let conn = Connection::open_with_flags_and_vfs(
            "file:webspec.db?immutable=1",
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
            "http",
        )
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
        CONN.with(|slot| *slot.borrow_mut() = Some(conn));
        Ok(())
    }

    #[wasm_bindgen]
    pub fn handle(request_json: &str) -> String {
        CONN.with(|slot| match slot.borrow().as_ref() {
            Some(conn) => webspec_index::api::handle_json(conn, request_json),
            None => {
                r#"{"type":"error","code":"not_open","message":"call open(manifest_url) first"}"#
                    .to_string()
            }
        })
    }

    #[wasm_bindgen]
    pub fn stats() -> String {
        serde_json::to_string(&vfs::stats("webspec.db")).unwrap_or_else(|_| "null".into())
    }
}
