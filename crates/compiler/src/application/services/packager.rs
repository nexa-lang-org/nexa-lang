use crate::domain::ast::Program;

/// Fixed 3-byte magic that identifies a Nexa bundle.
const NXB_MAGIC: &[u8; 3] = b"NXB";

/// Format version stored in the 4th header byte.
/// Increment this whenever the `Program` AST layout changes in a breaking way.
const NXB_FORMAT_VERSION: u8 = 1;

#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    #[error("invalid NXB magic header — this is not a Nexa bundle")]
    BadMagic,
    #[error(
        "unsupported NXB format version {found} \
         (this CLI supports version {supported}); please recompile your bundle"
    )]
    FormatVersion { found: u8, supported: u8 },
    #[error("bincode decode error: {0}")]
    Decode(#[from] bincode::error::DecodeError),
    #[error("bincode encode error: {0}")]
    Encode(#[from] bincode::error::EncodeError),
}

/// Serialize a `Program` to NXB binary format.
///
/// Layout: `b"NXB"` (3 bytes) + `NXB_FORMAT_VERSION` (1 byte) + bincode payload.
pub fn encode_nxb(program: &Program) -> Result<Vec<u8>, PackageError> {
    let config = bincode::config::standard();
    let payload = bincode::serde::encode_to_vec(program, config)?;
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(NXB_MAGIC);
    out.push(NXB_FORMAT_VERSION);
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Deserialize a `Program` from NXB binary format.
///
/// Returns `PackageError::BadMagic` if the first 3 bytes are not `b"NXB"`.
/// Returns `PackageError::FormatVersion` if the version byte is not recognised,
/// with a human-readable message telling the user to recompile.
pub fn decode_nxb(bytes: &[u8]) -> Result<Program, PackageError> {
    if bytes.len() < 4 || &bytes[..3] != NXB_MAGIC.as_slice() {
        return Err(PackageError::BadMagic);
    }
    let format_ver = bytes[3];
    if format_ver != NXB_FORMAT_VERSION {
        return Err(PackageError::FormatVersion {
            found: format_ver,
            supported: NXB_FORMAT_VERSION,
        });
    }
    // bincode v2: decode_from_slice always returns Result — it never panics on
    // corrupt data. Any malformed payload is propagated as PackageError::Decode.
    let config = bincode::config::standard();
    let (program, _): (Program, usize) = bincode::serde::decode_from_slice(&bytes[4..], config)?;
    Ok(program)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ast::ServerConfig;

    fn minimal_program() -> Program {
        Program {
            name: "test".into(),
            package: None,
            imports: vec![],
            server: Some(ServerConfig { port: 3000 }),
            declarations: vec![],
            routes: vec![],
        }
    }

    #[test]
    fn roundtrip_empty_program() {
        let prog = minimal_program();
        let bytes = encode_nxb(&prog).unwrap();
        // Header: b"NXB" + version byte 0x01
        assert!(bytes.starts_with(b"NXB\x01"));
        let decoded = decode_nxb(&bytes).unwrap();
        assert_eq!(decoded.name, "test");
        assert_eq!(decoded.server.unwrap().port, 3000);
    }

    #[test]
    fn decode_bad_magic_returns_error() {
        let bad = b"XXXX\x00\x00\x00\x00";
        assert!(matches!(decode_nxb(bad), Err(PackageError::BadMagic)));
    }

    #[test]
    fn decode_too_short_returns_error() {
        assert!(matches!(decode_nxb(b"NXB"), Err(PackageError::BadMagic)));
    }

    #[test]
    fn decode_unsupported_version_returns_error() {
        // Simulate a future bundle with version 99.
        let mut bytes = encode_nxb(&minimal_program()).unwrap();
        bytes[3] = 99;
        match decode_nxb(&bytes) {
            Err(PackageError::FormatVersion { found: 99, supported: 1 }) => {}
            other => panic!("expected FormatVersion error, got {other:?}"),
        }
    }

    #[test]
    fn decode_version_zero_returns_error() {
        // Version 0 predates the versioning scheme.
        let mut bytes = encode_nxb(&minimal_program()).unwrap();
        bytes[3] = 0;
        assert!(matches!(
            decode_nxb(&bytes),
            Err(PackageError::FormatVersion { found: 0, .. })
        ));
    }

    #[test]
    fn decode_corrupted_payload_returns_error_not_panic() {
        // Valid magic + version, but payload is garbage bytes.
        // bincode v2 must return Err(Decode), never panic.
        let corrupt = b"NXB\x01\xff\xff\xff\xff\xff\xff\xff\xff";
        assert!(matches!(decode_nxb(corrupt), Err(PackageError::Decode(_))));
    }
}
