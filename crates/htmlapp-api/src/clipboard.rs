//! `clipboard` — text, HTML, images, and file lists (PRD §9.3 Tier 3).
//!
//! Read and write are separate grants because they are different risks: writing places content the
//! user can see, while reading exposes whatever they copied for any other reason while the document
//! was running.

use htmlapp_bridge::RpcError;
use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture};
use htmlapp_caps::ClipboardAccess;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::context::Ctx;
use crate::params::decode;

pub struct ClipboardModule {
    ctx: Ctx,
}

#[derive(Deserialize)]
struct TextParams {
    text: String,
}

#[derive(Deserialize)]
struct HtmlParams {
    html: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct FilesParams {
    paths: Vec<String>,
}

impl ClipboardModule {
    pub fn new(ctx: Ctx) -> Self {
        Self { ctx }
    }

    #[cfg(feature = "tier3")]
    fn open(&self) -> Result<arboard::Clipboard, RpcError> {
        arboard::Clipboard::new().map_err(|e| {
            RpcError::new(
                htmlapp_bridge::ErrorCode::OperationFailed,
                format!("could not reach the clipboard: {e}"),
            )
        })
    }
}

impl ApiHandler for ClipboardModule {
    fn name(&self) -> &'static str {
        "clipboard"
    }

    #[cfg(feature = "tier3")]
    fn invoke<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            match method {
                "readText" => {
                    self.ctx.check_clipboard(ClipboardAccess::Read)?;
                    Ok(json!(self.open()?.get_text().unwrap_or_default()))
                }
                "writeText" => {
                    self.ctx.check_clipboard(ClipboardAccess::Write)?;
                    let params: TextParams = decode("clipboard.writeText", params)?;
                    self.open()?.set_text(params.text).map_err(|e| {
                        RpcError::new(htmlapp_bridge::ErrorCode::OperationFailed, e.to_string())
                    })?;
                    Ok(Value::Null)
                }
                "readHtml" => {
                    self.ctx.check_clipboard(ClipboardAccess::Read)?;
                    // arboard has no HTML read; text is the honest answer rather than an error.
                    Ok(json!(self.open()?.get_text().unwrap_or_default()))
                }
                "writeHtml" => {
                    self.ctx.check_clipboard(ClipboardAccess::Write)?;
                    let params: HtmlParams = decode("clipboard.writeHtml", params)?;
                    self.open()?
                        .set_html(params.html, params.text)
                        .map_err(|e| RpcError::new(
                            htmlapp_bridge::ErrorCode::OperationFailed, e.to_string()))?;
                    Ok(Value::Null)
                }
                "readImage" => {
                    self.ctx.check_clipboard(ClipboardAccess::Read)?;
                    let image = self.open()?.get_image().map_err(|e| {
                        RpcError::new(htmlapp_bridge::ErrorCode::OperationFailed, e.to_string())
                    })?;
                    // Handed back as a data URI so the page can put it straight into an <img>.
                    let png = encode_png(&image)?;
                    use base64::Engine as _;
                    Ok(json!(format!(
                        "data:image/png;base64,{}",
                        base64::engine::general_purpose::STANDARD.encode(png)
                    )))
                }
                "writeImage" => {
                    self.ctx.check_clipboard(ClipboardAccess::Write)?;
                    let _ = params;
                    Err(RpcError::unsupported(
                        "writing images to the clipboard is not implemented yet",
                    ))
                }
                "readFiles" => {
                    self.ctx.check_clipboard(ClipboardAccess::Read)?;
                    // A copied file list arrives as newline-separated URIs on both X11 and Wayland.
                    let text = self.open()?.get_text().unwrap_or_default();
                    let paths: Vec<String> = text
                        .lines()
                        .filter_map(|line| line.strip_prefix("file://"))
                        .map(str::to_string)
                        .collect();
                    Ok(json!(paths))
                }
                "writeFiles" => {
                    self.ctx.check_clipboard(ClipboardAccess::Write)?;
                    let params: FilesParams = decode("clipboard.writeFiles", params)?;
                    // Only paths the document may read can be advertised to other applications.
                    let mut uris = Vec::new();
                    for path in &params.paths {
                        let checked = self.ctx.check_read(path)?;
                        uris.push(format!("file://{}", checked.display()));
                    }
                    self.open()?.set_text(uris.join("\n")).map_err(|e| {
                        RpcError::new(htmlapp_bridge::ErrorCode::OperationFailed, e.to_string())
                    })?;
                    Ok(Value::Null)
                }
                other => Err(RpcError::not_found(&format!("clipboard.{other}"))),
            }
        })
    }

    #[cfg(not(feature = "tier3"))]
    fn invoke<'a>(&'a self, _method: &'a str, _params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async { Err(RpcError::unsupported("this build has no clipboard support")) })
    }
}

/// Encode a clipboard image as PNG without pulling in an image codec crate.
#[cfg(feature = "tier3")]
fn encode_png(image: &arboard::ImageData<'_>) -> Result<Vec<u8>, RpcError> {
    // arboard hands back raw RGBA. A minimal, uncompressed-deflate PNG keeps this dependency-free.
    png::encode_rgba(image.width as u32, image.height as u32, &image.bytes)
        .ok_or_else(|| RpcError::internal("could not encode the clipboard image"))
}

#[cfg(feature = "tier3")]
mod png {
    /// CRC-32 as specified by PNG.
    fn crc32(data: &[u8]) -> u32 {
        let mut table = [0u32; 256];
        for (i, entry) in table.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *entry = c;
        }
        let mut crc = 0xFFFF_FFFFu32;
        for byte in data {
            crc = table[((crc ^ *byte as u32) & 0xFF) as usize] ^ (crc >> 8);
        }
        crc ^ 0xFFFF_FFFF
    }

    fn adler32(data: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for byte in data {
            a = (a + *byte as u32) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }

    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        let mut with_kind = kind.to_vec();
        with_kind.extend_from_slice(body);
        out.extend_from_slice(&with_kind);
        out.extend_from_slice(&crc32(&with_kind).to_be_bytes());
    }

    /// Write RGBA as a PNG using stored (uncompressed) deflate blocks.
    pub fn encode_rgba(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
        if rgba.len() < (width as usize) * (height as usize) * 4 {
            return None;
        }

        // Each scanline is prefixed with filter type 0.
        let mut raw = Vec::with_capacity(rgba.len() + height as usize);
        for y in 0..height as usize {
            raw.push(0);
            let start = y * width as usize * 4;
            raw.extend_from_slice(&rgba[start..start + width as usize * 4]);
        }

        // zlib header, then stored deflate blocks of at most 65535 bytes.
        let mut z = vec![0x78, 0x01];
        for (i, block) in raw.chunks(65_535).enumerate() {
            let last = (i + 1) * 65_535 >= raw.len();
            z.push(if last { 1 } else { 0 });
            z.extend_from_slice(&(block.len() as u16).to_le_bytes());
            z.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
            z.extend_from_slice(block);
        }
        z.extend_from_slice(&adler32(&raw).to_be_bytes());

        let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
        chunk(&mut out, b"IHDR", &ihdr);
        chunk(&mut out, b"IDAT", &z);
        chunk(&mut out, b"IEND", &[]);
        Some(out)
    }
}
