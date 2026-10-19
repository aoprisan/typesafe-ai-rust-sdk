//! Images, for Jev 1.13 and later. Experimental: TypeSafe calls them a danger-zone feature, not
//! production-ready, and a request that carries one is sent with
//! `TypeSafe-Danger-Zone-Images: true`.
//!
//! Each image goes once into the request's assets, under a reference string of your choosing,
//! and the model sees it wherever that reference is written: in the state, an instruction or a
//! criterion. Every reference must be used somewhere.
//!
//! ```no_run
//! # async fn run() -> typesafe::Result<()> {
//! use typesafe::{BoxQuestion, Client, Image, Noul, Questions, json};
//!
//! let client = Client::from_env()?;
//! let res = client
//!     .system_one(
//!         json!({"photo": "photo", "caption": "Our new office"}),
//!         Questions::new()
//!             .with("has_people", Noul::new("photo shows at least one person"))
//!             .with("door", BoxQuestion::new("the front door in photo")),
//!     )
//!     .model("jev-1.14.0")
//!     .image("photo", Image::from_bytes(std::fs::read("office.jpg").unwrap())?)
//!     .await?;
//! # Ok(()) }
//! ```

use serde::ser::{SerializeStruct, Serializer};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The image formats the API reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ImageFormat {
    /// `"png"`.
    Png,
    /// `"jpeg"`.
    Jpeg,
}

/// An image, base64-encoded, as it goes into a request's assets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    format: ImageFormat,
    data_base64: String,
}

impl Image {
    /// An image from its bytes, its format read from them.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] if the bytes are neither a PNG nor a JPEG. Size limits (256 to
    /// 1568 pixels a side) are left to the API.
    pub fn from_bytes(bytes: impl AsRef<[u8]>) -> Result<Self> {
        let bytes = bytes.as_ref();
        let format = sniff(bytes)
            .ok_or_else(|| Error::invalid_request("an image must be a PNG or a JPEG"))?;
        Ok(Self {
            format,
            data_base64: base64(bytes),
        })
    }

    /// An image already base64-encoded, e.g. read from a data URL.
    pub fn from_base64(data_base64: impl Into<String>, format: ImageFormat) -> Self {
        Self {
            format,
            data_base64: data_base64.into(),
        }
    }

    /// The image's format.
    pub fn format(&self) -> ImageFormat {
        self.format
    }

    /// The image's bytes, base64-encoded.
    pub fn data_base64(&self) -> &str {
        &self.data_base64
    }
}

/// `{"type": "image", "data_base64": "…", "format": "png"}`.
impl Serialize for Image {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("Image", 3)?;
        st.serialize_field("type", "image")?;
        st.serialize_field("data_base64", &self.data_base64)?;
        st.serialize_field("format", &self.format)?;
        st.end()
    }
}

fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(ImageFormat::Jpeg)
    } else {
        None
    }
}

/// Standard base64 with padding: the one thing the crate needs it for does not justify a
/// dependency.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[(n >> (18 - 6 * i) & 0x3f) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn base64_matches_the_rfc_vectors() {
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(plain.as_bytes()), encoded);
        }
    }

    #[test]
    fn reads_the_format_from_the_bytes() {
        let png = Image::from_bytes([0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0]).unwrap();
        assert_eq!(png.format(), ImageFormat::Png);
        assert_eq!(
            serde_json::to_value(&png).unwrap(),
            json!({"type": "image", "data_base64": "iVBORw0KGgoA", "format": "png"})
        );
        let jpeg = Image::from_bytes([0xff, 0xd8, 0xff, 0xe0]).unwrap();
        assert_eq!(jpeg.format(), ImageFormat::Jpeg);
        assert!(matches!(
            Image::from_bytes(b"GIF89a"),
            Err(Error::InvalidRequest { .. })
        ));
    }
}
