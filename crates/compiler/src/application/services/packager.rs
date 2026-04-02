use crate::domain::ast::Program;

const NXB_MAGIC: &[u8; 4] = b"NXB\x01";

#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    #[error("invalid NXB magic header")]
    BadMagic,
    #[error("bincode decode error: {0}")]
    Decode(#[from] bincode::error::DecodeError),
    #[error("bincode encode error: {0}")]
    Encode(#[from] bincode::error::EncodeError),
}

/// Serialize a `Program` to NXB binary format: 4-byte magic + bincode payload.
pub fn encode_nxb(program: &Program) -> Result<Vec<u8>, PackageError> {
    let config = bincode::config::standard();
    let payload = bincode::serde::encode_to_vec(program, config)?;
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(NXB_MAGIC);
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Deserialize a `Program` from NXB binary format.
pub fn decode_nxb(bytes: &[u8]) -> Result<Program, PackageError> {
    if bytes.len() < 4 || &bytes[..4] != NXB_MAGIC {
        return Err(PackageError::BadMagic);
    }
    let config = bincode::config::standard();
    let (program, _): (Program, usize) =
        bincode::serde::decode_from_slice(&bytes[4..], config)?;
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
}
