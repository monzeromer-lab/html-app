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
struct ImageParams {
    data: String,
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
                    let params: ImageParams = decode("clipboard.writeImage", params)?;
                    let (width, height, bytes) = decode_data_uri(&params.data)?;
                    self.open()?
                        .set_image(arboard::ImageData {
                            width,
                            height,
                            bytes: std::borrow::Cow::Owned(bytes),
                        })
                        .map_err(|e| RpcError::new(
                            htmlapp_bridge::ErrorCode::OperationFailed, e.to_string()))?;
                    Ok(Value::Null)
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

/// Encode a clipboard image as PNG.
#[cfg(feature = "tier3")]
fn encode_png(image: &arboard::ImageData<'_>) -> Result<Vec<u8>, RpcError> {
    let mut out = Vec::new();
    {
        let mut encoder =
            png::Encoder::new(&mut out, image.width as u32, image.height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| RpcError::internal(format!("could not encode PNG: {e}")))?;
        writer
            .write_image_data(&image.bytes)
            .map_err(|e| RpcError::internal(format!("could not encode PNG: {e}")))?;
    }
    Ok(out)
}

/// Decode a `data:` URI into the RGBA buffer arboard wants.
///
/// Only PNG is accepted. A clipboard image arriving as an arbitrary URL would mean fetching it,
/// which is `http`'s business and governed by a different grant.
#[cfg(feature = "tier3")]
fn decode_data_uri(data: &str) -> Result<(usize, usize, Vec<u8>), RpcError> {
    use base64::Engine as _;

    let (header, payload) = data
        .strip_prefix("data:")
        .and_then(|rest| rest.split_once(','))
        .ok_or_else(|| RpcError::invalid_params("expected a data: URI"))?;

    if !header.starts_with("image/png") {
        return Err(RpcError::invalid_params(
            "only image/png data URIs can be written to the clipboard",
        ));
    }

    let bytes = if header.ends_with(";base64") {
        base64::engine::general_purpose::STANDARD
            .decode(payload.as_bytes())
            .map_err(|e| RpcError::invalid_params(format!("not base64: {e}")))?
    } else {
        payload.as_bytes().to_vec()
    };

    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder
        .read_info()
        .map_err(|e| RpcError::invalid_params(format!("not a valid PNG: {e}")))?;

    let mut buffer = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|e| RpcError::invalid_params(format!("could not decode the PNG: {e}")))?;
    buffer.truncate(info.buffer_size());

    // arboard wants RGBA; widen anything narrower rather than refusing it.
    let rgba = match info.color_type {
        png::ColorType::Rgba => buffer,
        png::ColorType::Rgb => buffer
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::Grayscale => buffer.iter().flat_map(|g| [*g, *g, *g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => buffer
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        other => {
            return Err(RpcError::invalid_params(format!(
                "{other:?} PNGs are not supported"
            )));
        }
    };

    Ok((info.width as usize, info.height as usize, rgba))
}
