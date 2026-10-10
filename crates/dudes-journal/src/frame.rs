//! The frame around a record.
//!
//! ```text
//! len: u32 LE · ver: u16 LE · kind: u16 LE · crc32c: u32 LE · payload (len bytes)
//! ```
//!
//! The 12-byte header is fixed. `crc32c` is CRC-32/ISCSI (Castagnoli) over `ver ‖ kind ‖ payload`,
//! the two fields as their little-endian bytes. Compatible frames skip unknown kinds. Required
//! frames refuse them so replay cannot silently omit their state.

use crc::{CRC_32_ISCSI, Crc};
use dudes_proto::RecordKind;

/// Bytes before the payload.
pub const HEADER_LEN: usize = 12;
/// The compatible header version written by the default append path.
pub const VERSION: u16 = 1;
/// The header version whose record kinds must be understood by the reader.
pub const REQUIRED_VERSION: u16 = 2;
/// The largest on-disk payload a frame may carry: 16 MiB.
///
/// This is the persisted read/format ceiling, deliberately wider than the delivery admission at
/// [`dudes_proto::rpc::MAX_FRAME_BYTES`]. Readers continue to accept existing frames through this
/// boundary; [`crate::Writer`] applies the stricter exact-notification limit to new appends.
pub const MAX_PAYLOAD: usize = 16 * 1024 * 1024;

const CRC: Crc<u32> = Crc::<u32>::new(&CRC_32_ISCSI);

/// Why a byte range is not a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    /// Fewer bytes than the header, or than the header claims.
    #[error("short frame: {need} bytes needed")]
    Short {
        /// Bytes the complete frame would occupy.
        need: usize,
    },
    /// `len` exceeds [`MAX_PAYLOAD`]; the header is not one a writer produces.
    #[error("frame length {len} exceeds the {MAX_PAYLOAD}-byte payload limit")]
    TooLong {
        /// The claimed payload length.
        len: u32,
    },
    /// The checksum does not match `ver ‖ kind ‖ payload`.
    #[error("frame crc32c mismatch: header {expected:#010x}, computed {found:#010x}")]
    Crc {
        /// The checksum in the header.
        expected: u32,
        /// The checksum of the bytes.
        found: u32,
    },
    /// An intact frame with a header version this build does not read.
    #[error("frame version {found} is not supported")]
    Version {
        /// The version in the header.
        found: u16,
    },
}

/// Why this build refuses to interpret an intact frame's meaning: a [`REQUIRED_VERSION`] frame
/// names a record kind this build does not recognize. Kept distinct from
/// [`FrameError`], which is a publicly exhaustive enum: the frame here is byte-for-byte intact —
/// never corrupt, never torn — so this is a different question than anything `FrameError`
/// answers, not a fifth answer to the same one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("required frame record kind {kind} is not understood")]
pub struct UnknownRequiredKind {
    /// The unrecognized kind tag.
    pub kind: u16,
}

/// Whether this build must understand `kind` at `ver`, and does. [`VERSION`] frames carry no such
/// requirement — an unrecognized kind there is skipped by the journal reader, not refused.
/// [`REQUIRED_VERSION`] frames do: replay must not silently omit the record they carry.
pub fn check_required_kind(ver: u16, kind: u16) -> Result<(), UnknownRequiredKind> {
    if ver == REQUIRED_VERSION && RecordKind::from_tag(kind).is_none() {
        return Err(UnknownRequiredKind { kind });
    }
    Ok(())
}

/// The decoded 12-byte header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// Payload length in bytes.
    pub len: u32,
    /// Format version.
    pub ver: u16,
    /// The record kind tag (`RecordKind::tag()`).
    pub kind: u16,
    /// crc32c over `ver ‖ kind ‖ payload`.
    pub crc: u32,
}

impl Header {
    /// Read the fixed header. Does not validate anything.
    #[must_use]
    pub fn parse(bytes: &[u8; HEADER_LEN]) -> Self {
        Self {
            len: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            ver: u16::from_le_bytes([bytes[4], bytes[5]]),
            kind: u16::from_le_bytes([bytes[6], bytes[7]]),
            crc: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
        }
    }

    /// The header as it is written.
    #[must_use]
    pub fn to_bytes(self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[0..4].copy_from_slice(&self.len.to_le_bytes());
        out[4..6].copy_from_slice(&self.ver.to_le_bytes());
        out[6..8].copy_from_slice(&self.kind.to_le_bytes());
        out[8..12].copy_from_slice(&self.crc.to_le_bytes());
        out
    }

    /// Bytes the whole frame occupies: header plus the claimed payload.
    ///
    /// Valid only for a header whose `len` has been checked against [`MAX_PAYLOAD`] — every
    /// caller here checks first ([`decode`], and the reader's frame step and frame search).
    /// `HEADER_LEN + len` is `usize` arithmetic, and on a 32-bit target a `len` near
    /// [`u32::MAX`] overflows it: a panic in debug, a wrapped extent in release. For the
    /// extent an *unchecked* header claims — any twelve bytes off a disk — use
    /// `Header::claimed_len` (crate-internal), which is `u64` on every target.
    #[must_use]
    pub fn frame_len(self) -> usize {
        HEADER_LEN + self.len as usize
    }

    /// Bytes the frame *claims* to occupy, for a header that has not been checked. Every width
    /// here is `u64`, so the largest header a disk can hold is arithmetic on any target rather
    /// than an overflow — which matters because torn-tail detection reads the claimed extent of
    /// headers it is about to reject (`reader::ScanFailure::claimed_end`).
    #[must_use]
    pub(crate) fn claimed_len(self) -> u64 {
        HEADER_LEN as u64 + u64::from(self.len)
    }

    /// Refuse a header no writer produces.
    fn check_len(self) -> Result<(), FrameError> {
        if self.len as usize > MAX_PAYLOAD {
            return Err(FrameError::TooLong { len: self.len });
        }
        Ok(())
    }
}

/// A frame that decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decoded<'a> {
    /// A supported format version, [`VERSION`] or [`REQUIRED_VERSION`].
    pub ver: u16,
    /// The record kind tag.
    pub kind: u16,
    /// The payload bytes.
    pub payload: &'a [u8],
    /// Bytes the frame occupied, header included.
    pub total_len: usize,
}

/// crc32c (CRC-32/ISCSI) over `ver ‖ kind ‖ payload`.
#[must_use]
pub fn crc32c(ver: u16, kind: u16, payload: &[u8]) -> u32 {
    let mut digest = CRC.digest();
    digest.update(&ver.to_le_bytes());
    digest.update(&kind.to_le_bytes());
    digest.update(payload);
    digest.finalize()
}

/// Append one frame of `kind` around `payload` to `out`; returns the bytes appended.
pub fn encode(kind: u16, payload: &[u8], out: &mut Vec<u8>) -> Result<usize, FrameError> {
    encode_version(VERSION, kind, payload, out)
}

/// Append a frame whose record kind every reader must understand.
pub fn encode_required(
    kind: RecordKind,
    payload: &[u8],
    out: &mut Vec<u8>,
) -> Result<usize, FrameError> {
    encode_version(REQUIRED_VERSION, kind.tag(), payload, out)
}

fn encode_version(
    version: u16,
    kind: u16,
    payload: &[u8],
    out: &mut Vec<u8>,
) -> Result<usize, FrameError> {
    if payload.len() > MAX_PAYLOAD {
        return Err(FrameError::TooLong {
            len: u32::try_from(payload.len()).unwrap_or(u32::MAX),
        });
    }
    let header = Header {
        len: u32::try_from(payload.len()).expect("checked against MAX_PAYLOAD"),
        ver: version,
        kind,
        crc: crc32c(version, kind, payload),
    };
    out.extend_from_slice(&header.to_bytes());
    out.extend_from_slice(payload);
    Ok(header.frame_len())
}

/// Check a header against the payload it claims: checksum first (integrity), then version
/// (meaning). A wrong checksum is never reported as a version problem.
pub fn verify(header: Header, payload: &[u8]) -> Result<(), FrameError> {
    let found = crc32c(header.ver, header.kind, payload);
    if found != header.crc {
        return Err(FrameError::Crc {
            expected: header.crc,
            found,
        });
    }
    if !supports_version(header.ver) {
        return Err(FrameError::Version { found: header.ver });
    }
    Ok(())
}

pub(crate) const fn supports_version(version: u16) -> bool {
    matches!(version, VERSION | REQUIRED_VERSION)
}

/// Decode the frame at the start of `bytes`. Never panics on any input; the fuzz target and
/// the torn-tail scanner both rest on that.
pub fn decode(bytes: &[u8]) -> Result<Decoded<'_>, FrameError> {
    let Some(head) = bytes.first_chunk::<HEADER_LEN>() else {
        return Err(FrameError::Short { need: HEADER_LEN });
    };
    let header = Header::parse(head);
    header.check_len()?;
    let total_len = header.frame_len();
    let Some(payload) = bytes.get(HEADER_LEN..total_len) else {
        return Err(FrameError::Short { need: total_len });
    };
    verify(header, payload)?;
    Ok(Decoded {
        ver: header.ver,
        kind: header.kind,
        payload,
        total_len,
    })
}

#[cfg(test)]
mod tests {
    use dudes_proto::rpc::MAX_FRAME_BYTES;
    use proptest::prelude::*;

    use super::*;

    /// Journal format: the checksum is CRC-32/ISCSI, pinned by the standard check value
    /// so a wrong polynomial or reflection fails here, not in a reader on another machine.
    #[test]
    fn crc32c_is_castagnoli() {
        assert_eq!(CRC.checksum(b"123456789"), 0xE306_9283);
    }

    /// Journal format: the header is 12 bytes, little-endian, in the documented order.
    #[test]
    fn header_layout_is_fixed() {
        let mut out = Vec::new();
        let n = encode(7, b"hi", &mut out).unwrap();
        assert_eq!(n, 14);
        assert_eq!(&out[0..4], &2u32.to_le_bytes());
        assert_eq!(&out[4..6], &1u16.to_le_bytes());
        assert_eq!(&out[6..8], &7u16.to_le_bytes());
        assert_eq!(&out[8..12], &crc32c(1, 7, b"hi").to_le_bytes());
        assert_eq!(&out[12..], b"hi");
    }

    /// A writer never produces a frame a reader cannot frame: payloads over 16 MiB are refused
    /// on encode, and a header claiming more is refused on decode.
    #[test]
    fn payload_limit_holds_both_ways() {
        assert_eq!(
            MAX_PAYLOAD,
            16 * MAX_FRAME_BYTES,
            "the 16 MiB persisted ceiling stays distinct from 1 MiB delivery admission"
        );
        let big = vec![0u8; MAX_PAYLOAD + 1];
        assert!(matches!(
            encode(1, &big, &mut Vec::new()),
            Err(FrameError::TooLong { .. })
        ));
        let mut ok = Vec::new();
        encode(1, &big[..MAX_PAYLOAD], &mut ok).unwrap();
        assert_eq!(decode(&ok).unwrap().payload.len(), MAX_PAYLOAD);
        let header = Header {
            len: u32::try_from(MAX_PAYLOAD).unwrap() + 1,
            ver: VERSION,
            kind: 1,
            crc: 0,
        };
        assert_eq!(
            decode(&header.to_bytes()),
            Err(FrameError::TooLong {
                len: u32::try_from(MAX_PAYLOAD).unwrap() + 1
            })
        );
    }

    /// Torn-tail contract: a frame cut anywhere reports `Short` with the length it needs,
    /// so the scanner can tell "the writer stopped here" from "the bytes are wrong".
    #[test]
    fn truncation_is_short_not_corrupt() {
        let mut out = Vec::new();
        encode(3, b"payload", &mut out).unwrap();
        for cut in 0..out.len() {
            let err = decode(&out[..cut]).unwrap_err();
            let need = if cut < HEADER_LEN {
                HEADER_LEN
            } else {
                out.len()
            };
            assert_eq!(err, FrameError::Short { need }, "cut at {cut}");
        }
    }

    /// Version skew is reported only on an intact frame: checksum before version.
    #[test]
    fn version_is_checked_after_crc() {
        let header = Header {
            len: 0,
            ver: 3,
            kind: 1,
            crc: crc32c(3, 1, b""),
        };
        assert_eq!(
            decode(&header.to_bytes()),
            Err(FrameError::Version { found: 3 })
        );
        let mut bad = header.to_bytes();
        bad[8] ^= 1;
        assert!(matches!(decode(&bad), Err(FrameError::Crc { .. })));
    }

    /// `decode` frames the bytes regardless of whether the kind is understood — that question is
    /// [`check_required_kind`]'s, kept off `FrameError`.
    #[test]
    fn decode_is_indifferent_to_kind_legitimacy() {
        let mut recognized = Vec::new();
        encode_required(RecordKind::Pause, b"{}", &mut recognized).unwrap();
        let unknown_kind = 0xFFFF;
        let mut unknown = Vec::new();
        encode_version(REQUIRED_VERSION, unknown_kind, b"{}", &mut unknown).unwrap();
        assert!(decode(&recognized).is_ok());
        assert!(decode(&unknown).is_ok());
        assert_eq!(check_required_kind(VERSION, unknown_kind), Ok(()));
        assert_eq!(
            check_required_kind(REQUIRED_VERSION, unknown_kind),
            Err(UnknownRequiredKind { kind: unknown_kind })
        );
    }

    proptest! {
        /// Journal format: encode then decode is the identity for any kind and payload,
        /// and the frame occupies exactly header plus payload.
        #[test]
        fn encode_decode_identity(kind in any::<u16>(), payload in proptest::collection::vec(any::<u8>(), 0..2048)) {
            let mut out = Vec::new();
            let n = encode(kind, &payload, &mut out).unwrap();
            prop_assert_eq!(n, out.len());
            let decoded = decode(&out).unwrap();
            prop_assert_eq!(decoded.kind, kind);
            prop_assert_eq!(decoded.ver, VERSION);
            prop_assert_eq!(decoded.payload, &payload[..]);
            prop_assert_eq!(decoded.total_len, HEADER_LEN + payload.len());
        }

        /// Integrity: flipping any single byte of header or payload fails decode.
        #[test]
        fn any_flipped_byte_fails(kind in any::<u16>(), payload in proptest::collection::vec(any::<u8>(), 0..256), index in any::<prop::sample::Index>(), bit in 0u8..8) {
            let mut out = Vec::new();
            encode(kind, &payload, &mut out).unwrap();
            let i = index.index(out.len());
            out[i] ^= 1 << bit;
            prop_assert!(decode(&out).is_err(), "byte {i} bit {bit}");
        }

        /// The decoder never panics on arbitrary bytes (the fuzz body, on stable).
        #[test]
        fn decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
            let _ = decode(&bytes);
        }
    }
}
