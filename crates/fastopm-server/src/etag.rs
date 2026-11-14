//! Express' default weak `ETag` (`etag@1` with `weak: true`), which `res.send` adds to every
//! body that has none yet: Verdaccio's error responses carry it.

/// `W/"<hex byte length>-<first 27 chars of base64(sha1(body))>"`.
pub(crate) fn weak_etag(body: &[u8]) -> String {
    let hash = base64(&sha1(body));
    format!("W/\"{:x}-{}\"", body.len(), &hash[..27])
}

/// SHA-1 (FIPS 180-4). Only used for the weak ETag above.
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    let mut message = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());
    for block in message.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp =
                a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        for (slot, value) in h.iter_mut().zip([a, b, c, d, e]) {
            *slot = slot.wrapping_add(value);
        }
    }
    let mut out = [0u8; 20];
    for (chunk, word) in out.chunks_exact_mut(4).zip(h) {
        chunk.copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// Standard base64 with padding.
fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[(n >> (18 - 6 * i)) as usize & 63]));
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

    #[test]
    fn sha1_vectors() {
        assert_eq!(hex::encode(sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(hex::encode(sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        let long = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(hex::encode(sha1(long)), "84983e441c3bd26ebaae4aa1f95129e5e54670f1");
        let million = vec![b'a'; 1_000_000];
        assert_eq!(hex::encode(sha1(&million)), "34aa973cd4c4daa4f61eeb2bdbad27316534016f");
    }

    #[test]
    fn base64_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
    }

    #[test]
    fn express_weak_etags() {
        // Values observed from verdaccio@7.1.0.
        assert_eq!(weak_etag(b""), "W/\"0-2jmj7l5rSw0yVb/vlWAYkK/YBwk\"");
        assert_eq!(
            weak_etag(b"{\n  \"error\": \"resource not found\"\n}\n"),
            "W/\"24-/Exrzi8zHCWw4+NVgc+l+JLt2TI\""
        );
        assert_eq!(
            weak_etag(b"{\n  \"error\": \"Web interface is disabled in the config file\"\n}\n"),
            "W/\"3e-XgDlLn0nsmbCcBuSD64gqtppKrs\""
        );
    }
}
