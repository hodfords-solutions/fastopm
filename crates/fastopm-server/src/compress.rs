//! Body encoders with the settings `compression@1.8` gives Node.js `zlib`: gzip and deflate
//! (zlib format) at level 6, brotli at quality 4 with a 22-bit window.

use std::io::Write;

use bytes::Bytes;

use crate::negotiate::Coding;

/// Encode a whole body.
pub(crate) fn encode(coding: Coding, body: &[u8]) -> Bytes {
    let out = match coding {
        Coding::Identity => return Bytes::copy_from_slice(body),
        Coding::Gzip => {
            let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
            enc.write_all(body).and_then(|()| enc.finish())
        }
        Coding::Deflate => {
            let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
            enc.write_all(body).and_then(|()| enc.finish())
        }
        Coding::Brotli => {
            let mut out = Vec::new();
            let params =
                brotli::enc::BrotliEncoderParams { quality: 4, lgwin: 22, ..Default::default() };
            brotli::BrotliCompress(&mut &body[..], &mut out, &params).map(|_| out)
        }
    };
    // Writing to a Vec cannot fail.
    Bytes::from(out.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    #[test]
    fn round_trips() {
        let body = "{\n  \"name\": \"x\"\n}\n".repeat(200);
        let gz = encode(Coding::Gzip, body.as_bytes());
        let mut out = String::new();
        flate2::read::GzDecoder::new(&gz[..]).read_to_string(&mut out).unwrap();
        assert_eq!(out, body);

        let zl = encode(Coding::Deflate, body.as_bytes());
        let mut out = String::new();
        flate2::read::ZlibDecoder::new(&zl[..]).read_to_string(&mut out).unwrap();
        assert_eq!(out, body);

        let br = encode(Coding::Brotli, body.as_bytes());
        let mut out = Vec::new();
        brotli::BrotliDecompress(&mut &br[..], &mut out).unwrap();
        assert_eq!(out, body.as_bytes());
        assert!(br.len() < body.len());
    }
}
