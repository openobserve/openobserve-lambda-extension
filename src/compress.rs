/// gzip helper shared by log + OTLP forwarders.
use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::Write;

/// Compress bytes with gzip using default compression level.
pub fn gzip(input: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut enc = GzEncoder::new(Vec::with_capacity(input.len() / 4), Compression::default());
    enc.write_all(input)?;
    enc.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use std::io::Read;

    #[test]
    fn roundtrip() {
        let input = b"hello world hello world hello world";
        let out = gzip(input).unwrap();
        assert!(out.len() < input.len() + 30);
        let mut dec = GzDecoder::new(&out[..]);
        let mut back = Vec::new();
        dec.read_to_end(&mut back).unwrap();
        assert_eq!(back, input);
    }
}
