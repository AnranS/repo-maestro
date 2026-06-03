use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web/dist/"]
struct WebAsset;

pub struct Asset {
    pub mime: &'static str,
    pub body: Vec<u8>,
}

pub fn asset(path: &str) -> Option<Asset> {
    let file = WebAsset::get(path).or_else(|| {
        if path == "index.html" {
            None
        } else {
            // SPA fallback: anything not found that's not an asset extension
            // (no dot) gets index.html — let the client router handle it.
            if !path.contains('.') {
                WebAsset::get("index.html")
            } else {
                None
            }
        }
    })?;

    let mime = mime_guess::from_path(path)
        .first_or_octet_stream()
        .essence_str()
        .to_string()
        .leak();

    Some(Asset {
        mime,
        body: file.data.into_owned(),
    })
}

/// Legacy alias kept for the index handler we kept around.
pub const INDEX_HTML: &str = include_str!("../../web/dist/index.html");
