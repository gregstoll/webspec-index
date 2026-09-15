use js_sys::Uint8Array;
use wasm_bindgen::JsValue;
use web_sys::{XmlHttpRequest, XmlHttpRequestResponseType};

use crate::layout::ChunkLayout;
use crate::source::{RangeSource, SourceError};

pub struct XhrSource {
    pub base_url: String,
}

impl RangeSource for XhrSource {
    fn fetch(
        &self,
        chunk: u32,
        local_start: u64,
        local_end_inclusive: u64,
    ) -> Result<Vec<u8>, SourceError> {
        let url = format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            ChunkLayout::chunk_name(chunk)
        );
        let xhr = XmlHttpRequest::new().map_err(js_err)?;
        xhr.open_with_async("GET", &url, false).map_err(js_err)?;
        xhr.set_request_header(
            "Range",
            &format!("bytes={local_start}-{local_end_inclusive}"),
        )
        .map_err(js_err)?;
        xhr.set_response_type(XmlHttpRequestResponseType::Arraybuffer);
        xhr.send().map_err(js_err)?;
        let status = xhr.status().map_err(js_err)?;
        if status != 206 {
            return Err(SourceError(format!(
                "range request to {url} returned HTTP {status}"
            )));
        }
        let buffer = xhr.response().map_err(js_err)?;
        let bytes = Uint8Array::new(&buffer).to_vec();
        let expected = (local_end_inclusive - local_start + 1) as usize;
        if bytes.len() != expected {
            return Err(SourceError(format!(
                "{url}: expected {expected} bytes, got {}",
                bytes.len()
            )));
        }
        Ok(bytes)
    }
}

pub fn fetch_text_sync(url: &str) -> Result<String, SourceError> {
    let xhr = XmlHttpRequest::new().map_err(js_err)?;
    xhr.open_with_async("GET", url, false).map_err(js_err)?;
    xhr.send().map_err(js_err)?;
    let status = xhr.status().map_err(js_err)?;
    if status != 200 {
        return Err(SourceError(format!("fetch {url} returned HTTP {status}")));
    }
    xhr.response_text()
        .map_err(js_err)?
        .ok_or_else(|| SourceError(format!("{url}: empty response")))
}

fn js_err(e: JsValue) -> SourceError {
    SourceError(format!("{e:?}"))
}
