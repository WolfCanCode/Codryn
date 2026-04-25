/// Compression threshold in bytes. Snippets smaller than this are stored uncompressed.
pub const COMPRESSION_THRESHOLD: usize = 1024;

/// Compression level for zstd (1-22, default 3 for speed).
pub const ZSTD_LEVEL: i32 = 3;

/// Prefix bytes to indicate compressed content.
pub const COMPRESSED_PREFIX: &[u8] = b"\x00ZSTD";

/// Compress a code snippet if it exceeds the threshold and compression reduces size.
/// Returns raw bytes for small content, or prefixed zstd-compressed bytes for large content.
pub fn maybe_compress(content: &str) -> Vec<u8> {
    if content.len() < COMPRESSION_THRESHOLD {
        return content.as_bytes().to_vec();
    }
    let compressed = zstd::encode_all(content.as_bytes(), ZSTD_LEVEL)
        .unwrap_or_else(|_| content.as_bytes().to_vec());

    // Only use compressed form if it's actually smaller
    if compressed.len() < content.len() {
        let mut result = Vec::with_capacity(COMPRESSED_PREFIX.len() + compressed.len());
        result.extend_from_slice(COMPRESSED_PREFIX);
        result.extend_from_slice(&compressed);
        result
    } else {
        content.as_bytes().to_vec()
    }
}

/// Decompress a code snippet, handling both compressed and uncompressed content.
/// Detects the COMPRESSED_PREFIX to decide whether to decompress.
pub fn maybe_decompress(data: &[u8]) -> String {
    if data.starts_with(COMPRESSED_PREFIX) {
        let compressed = &data[COMPRESSED_PREFIX.len()..];
        match zstd::decode_all(compressed) {
            Ok(decompressed) => String::from_utf8_lossy(&decompressed).into_owned(),
            Err(e) => {
                tracing::warn!(error = %e, "zstd decompression failed, returning raw");
                String::from_utf8_lossy(data).into_owned()
            }
        }
    } else {
        String::from_utf8_lossy(data).into_owned()
    }
}
