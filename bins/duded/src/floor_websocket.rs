//! The Floor gateway's private, bounded WebSocket codec.

pub(crate) const MAX_MESSAGE_BYTES: usize = 1_048_576;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Message {
    Text(String),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close,
}

#[derive(Debug, Default)]
pub(crate) struct Decoder {
    buffered: Vec<u8>,
    fragmented: Option<Vec<u8>>,
}

impl Decoder {
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<Vec<Message>, &'static str> {
        if self
            .buffered
            .len()
            .checked_add(bytes.len())
            .is_none_or(|len| len > MAX_MESSAGE_BYTES + 14)
        {
            return Err("WebSocket message exceeds 1 MiB");
        }
        self.buffered.extend_from_slice(bytes);
        let mut messages = Vec::new();
        while let Some((header, payload_len, opcode, final_frame)) = frame_head(&self.buffered)? {
            self.preflight(opcode, payload_len)?;
            let frame_len = header
                .checked_add(payload_len)
                .ok_or("WebSocket frame length overflow")?;
            if self.buffered.len() < frame_len {
                break;
            }
            let mask_at = header - 4;
            let mask = [
                self.buffered[mask_at],
                self.buffered[mask_at + 1],
                self.buffered[mask_at + 2],
                self.buffered[mask_at + 3],
            ];
            let mut payload = self.buffered[header..frame_len].to_vec();
            for (index, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[index % mask.len()];
            }
            self.buffered.drain(..frame_len);

            match opcode {
                0x0 => {
                    let mut complete = self
                        .fragmented
                        .take()
                        .expect("continuation preflight requires a fragment");
                    complete.extend_from_slice(&payload);
                    if final_frame {
                        messages.push(Message::Text(
                            String::from_utf8(complete)
                                .map_err(|_| "WebSocket text is not UTF-8")?,
                        ));
                    } else {
                        self.fragmented = Some(complete);
                    }
                }
                0x1 => {
                    debug_assert!(self.fragmented.is_none());
                    if final_frame {
                        messages.push(Message::Text(
                            String::from_utf8(payload)
                                .map_err(|_| "WebSocket text is not UTF-8")?,
                        ));
                    } else {
                        self.fragmented = Some(payload);
                    }
                }
                0x8 => {
                    if payload.len() == 1 {
                        return Err("invalid WebSocket close payload");
                    }
                    if payload.len() >= 2 {
                        let code = u16::from_be_bytes([payload[0], payload[1]]);
                        if !matches!(code, 1000..=1003 | 1007..=1014 | 3000..=4999) {
                            return Err("invalid WebSocket close code");
                        }
                    }
                    if payload.len() > 2 {
                        std::str::from_utf8(&payload[2..])
                            .map_err(|_| "WebSocket close reason is not UTF-8")?;
                    }
                    messages.push(Message::Close);
                }
                0x9 => messages.push(Message::Ping(payload)),
                0xa => messages.push(Message::Pong(payload)),
                _ => unreachable!("opcode preflight accepts the finite set"),
            }
        }
        if self.buffered.len() > MAX_MESSAGE_BYTES + 14 {
            return Err("WebSocket message exceeds 1 MiB");
        }
        Ok(messages)
    }

    fn preflight(&self, opcode: u8, payload_len: usize) -> Result<(), &'static str> {
        match opcode {
            0x0 => {
                let Some(fragment) = &self.fragmented else {
                    return Err("unexpected WebSocket continuation");
                };
                if fragment
                    .len()
                    .checked_add(payload_len)
                    .is_none_or(|len| len > MAX_MESSAGE_BYTES)
                {
                    return Err("WebSocket message exceeds 1 MiB");
                }
            }
            0x1 => {
                if self.fragmented.is_some() {
                    return Err("new WebSocket data frame during fragmentation");
                }
            }
            0x8..=0xa => {}
            _ => return Err("unsupported WebSocket opcode"),
        }
        Ok(())
    }
}

pub(crate) fn accept_key(key: &str) -> Option<String> {
    if !valid_client_key(key) {
        return None;
    }
    let mut input = Vec::with_capacity(key.len() + 36);
    input.extend_from_slice(key.as_bytes());
    input.extend_from_slice(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    Some(base64(&sha1(&input)))
}

pub(crate) fn text_frame(bytes: &[u8]) -> Option<Vec<u8>> {
    frame(0x1, bytes)
}

/// The payload of one whole text frame [`text_frame`] built, or `None` for anything else.
pub(crate) fn text_payload(frame: &[u8]) -> Option<&[u8]> {
    let Some((&[0x81, length], rest)) = frame.split_first_chunk::<2>() else {
        return None;
    };
    let (length, payload) = match length {
        0..=125 => (usize::from(length), rest),
        126 => {
            let (bytes, payload) = rest.split_first_chunk::<2>()?;
            (usize::from(u16::from_be_bytes(*bytes)), payload)
        }
        127 => {
            let (bytes, payload) = rest.split_first_chunk::<8>()?;
            (usize::try_from(u64::from_be_bytes(*bytes)).ok()?, payload)
        }
        _ => return None,
    };
    (payload.len() == length).then_some(payload)
}

pub(crate) fn pong_frame(bytes: &[u8]) -> Option<Vec<u8>> {
    (bytes.len() <= 125).then(|| frame(0xa, bytes)).flatten()
}

pub(crate) fn close_frame() -> Vec<u8> {
    vec![0x88, 0]
}

const POLICY_VIOLATION: u16 = 1008;

pub(crate) fn policy_close_frame() -> Vec<u8> {
    let [high, low] = POLICY_VIOLATION.to_be_bytes();
    vec![0x88, 2, high, low]
}

fn frame(opcode: u8, bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() > MAX_MESSAGE_BYTES {
        return None;
    }
    let mut output = Vec::with_capacity(bytes.len() + 10);
    output.push(0x80 | opcode);
    match bytes.len() {
        0..=125 => output.push(u8::try_from(bytes.len()).ok()?),
        126..=65_535 => {
            output.push(126);
            output.extend_from_slice(&u16::try_from(bytes.len()).ok()?.to_be_bytes());
        }
        _ => {
            output.push(127);
            output.extend_from_slice(&u64::try_from(bytes.len()).ok()?.to_be_bytes());
        }
    }
    output.extend_from_slice(bytes);
    Some(output)
}

fn frame_head(bytes: &[u8]) -> Result<Option<(usize, usize, u8, bool)>, &'static str> {
    if bytes.len() < 2 {
        return Ok(None);
    }
    let first = bytes[0];
    let second = bytes[1];
    if first & 0x70 != 0 {
        return Err("WebSocket RSV bits are unsupported");
    }
    if second & 0x80 == 0 {
        return Err("browser WebSocket frame is not masked");
    }
    let final_frame = first & 0x80 != 0;
    let opcode = first & 0x0f;
    let control = opcode & 0x08 != 0;
    if control && !final_frame {
        return Err("fragmented WebSocket control frame");
    }
    let code = second & 0x7f;
    let (length_bytes, payload_len) = match code {
        0..=125 => (0, usize::from(code)),
        126 => {
            if bytes.len() < 4 {
                return Ok(None);
            }
            let len = usize::from(u16::from_be_bytes([bytes[2], bytes[3]]));
            if len < 126 {
                return Err("non-canonical WebSocket frame length");
            }
            (2, len)
        }
        127 => {
            if bytes.len() < 10 {
                return Ok(None);
            }
            let raw = u64::from_be_bytes(bytes[2..10].try_into().expect("eight bytes checked"));
            if u16::try_from(raw).is_ok() || raw >> 63 != 0 {
                return Err("non-canonical WebSocket frame length");
            }
            let len = usize::try_from(raw).map_err(|_| "WebSocket frame length overflow")?;
            (8, len)
        }
        _ => unreachable!("the high mask bit was removed"),
    };
    if control && payload_len > 125 {
        return Err("WebSocket control frame exceeds 125 bytes");
    }
    if !control && payload_len > MAX_MESSAGE_BYTES {
        return Err("WebSocket message exceeds 1 MiB");
    }
    let header = 2 + length_bytes + 4;
    if bytes.len() < header {
        return Ok(None);
    }
    Ok(Some((header, payload_len, opcode, final_frame)))
}

fn valid_client_key(key: &str) -> bool {
    let bytes = key.as_bytes();
    bytes.len() == 24
        && bytes[22..] == *b"=="
        && bytes[..22].iter().all(|byte| base64_value(*byte).is_some())
        && base64_value(bytes[21]).is_some_and(|value| value.trailing_zeros() >= 4)
}

pub(crate) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(char::from(ALPHABET[usize::from(first >> 2)]));
        output.push(char::from(
            ALPHABET[usize::from((first & 0x03) << 4 | second >> 4)],
        ));
        if chunk.len() > 1 {
            output.push(char::from(
                ALPHABET[usize::from((second & 0x0f) << 2 | third >> 6)],
            ));
        } else {
            output.push('=');
        }
        if chunk.len() > 2 {
            output.push(char::from(ALPHABET[usize::from(third & 0x3f)]));
        } else {
            output.push('=');
        }
    }
    output
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

#[expect(
    clippy::many_single_char_names,
    reason = "SHA-1's five working words use the names from the algorithm"
)]
fn sha1(bytes: &[u8]) -> [u8; 20] {
    let bit_len = u64::try_from(bytes.len())
        .expect("an in-memory WebSocket key length fits u64")
        .wrapping_mul(8);
    let mut padded = bytes.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    let mut h = [
        0x6745_2301_u32,
        0xefcd_ab89,
        0x98ba_dcfe,
        0x1032_5476,
        0xc3d2_e1f0,
    ];
    for block in padded.as_chunks::<64>().0 {
        let mut words = [0_u32; 80];
        for (index, word) in words[..16].iter_mut().enumerate() {
            let at = index * 4;
            *word = u32::from_be_bytes(block[at..at + 4].try_into().expect("four bytes"));
        }
        for index in 16..80 {
            words[index] =
                (words[index - 3] ^ words[index - 8] ^ words[index - 14] ^ words[index - 16])
                    .rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (index, word) in words.into_iter().enumerate() {
            let (function, constant) = match index {
                0..=19 => ((b & c) | ((!b) & d), 0x5a82_7999),
                20..=39 => (b ^ c ^ d, 0x6ed9_eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
                _ => (b ^ c ^ d, 0xca62_c1d6),
            };
            let next = a
                .rotate_left(5)
                .wrapping_add(function)
                .wrapping_add(e)
                .wrapping_add(constant)
                .wrapping_add(word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = next;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut output = [0_u8; 20];
    for (chunk, word) in output.as_chunks_mut::<4>().0.iter_mut().zip(h) {
        chunk.copy_from_slice(&word.to_be_bytes());
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn masked_frame(first: u8, payload: &[u8]) -> Vec<u8> {
        let mask = [1_u8, 2, 3, 4];
        let mut frame = vec![first];
        match payload.len() {
            0..=125 => frame.push(0x80 | u8::try_from(payload.len()).unwrap()),
            126..=65_535 => {
                frame.push(0x80 | 0x7e);
                frame.extend_from_slice(&u16::try_from(payload.len()).unwrap().to_be_bytes());
            }
            _ => {
                frame.push(0x80 | 0x7f);
                frame.extend_from_slice(&u64::try_from(payload.len()).unwrap().to_be_bytes());
            }
        }
        frame.extend_from_slice(&mask);
        frame.extend(
            payload
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ mask[index % mask.len()]),
        );
        frame
    }

    fn masked_header(first: u8, payload_len: usize) -> Vec<u8> {
        let mut frame = masked_frame(first, &vec![0; payload_len]);
        frame.truncate(frame.len() - payload_len);
        frame
    }

    #[test]
    fn browser_handshake_and_masked_frames_match_known_vectors() {
        assert_eq!(
            sha1(b"abc"),
            [
                0xa9, 0x99, 0x3e, 0x36, 0x47, 0x06, 0x81, 0x6a, 0xba, 0x3e, 0x25, 0x71, 0x78, 0x50,
                0xc2, 0x6c, 0x9c, 0xd0, 0xd8, 0x9d,
            ]
        );
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ==").as_deref(),
            Some("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")
        );
        assert_eq!(accept_key("dGhlIHNhbXBsZSBub25jZQ="), None);

        let mut decoder = Decoder::default();
        assert_eq!(
            decoder
                .push(&[
                    0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58
                ])
                .unwrap(),
            vec![Message::Text("Hello".to_owned())]
        );

        let mut fragmented = Decoder::default();
        assert!(
            fragmented
                .push(&[0x01, 0x82, 1, 2, 3, 4, b'h' ^ 1, b'e' ^ 2])
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            fragmented
                .push(&[0x80, 0x83, 5, 6, 7, 8, b'l' ^ 5, b'l' ^ 6, b'o' ^ 7])
                .unwrap(),
            vec![Message::Text("hello".to_owned())]
        );

        assert_eq!(text_frame(b"ok"), Some(vec![0x81, 2, b'o', b'k']));
        assert_eq!(
            text_frame(&vec![0; MAX_MESSAGE_BYTES])
                .expect("the exact message bound")
                .len(),
            MAX_MESSAGE_BYTES + 10
        );
        assert!(text_frame(&vec![0; MAX_MESSAGE_BYTES + 1]).is_none());
        assert!(Decoder::default().push(&[0x81, 1, b'x']).is_err());
        assert!(Decoder::default().push(&[0x82, 0x80, 1, 2, 3, 4]).is_err());
        assert!(Decoder::default().push(&[0x09, 0xfe]).is_err());
        assert!(
            Decoder::default()
                .push(&[0x81, 0xfe, 0, 125, 1, 2, 3, 4])
                .is_err()
        );
        assert!(
            Decoder::default()
                .push(&[0x88, 0x82, 1, 2, 3, 4, 1 ^ 1, 0xed ^ 2])
                .is_err()
        );
    }

    #[test]
    fn fragment_headers_refuse_overflow_before_body() {
        assert!(Decoder::default().push(&masked_header(0x80, 1)).is_err());

        let mut during_fragment = Decoder::default();
        assert!(
            during_fragment
                .push(&masked_frame(0x01, b"a"))
                .unwrap()
                .is_empty()
        );
        assert!(during_fragment.push(&masked_header(0x81, 1)).is_err());

        let mut overflow = Decoder::default();
        assert!(
            overflow
                .push(&masked_frame(0x01, &vec![b'x'; MAX_MESSAGE_BYTES]))
                .unwrap()
                .is_empty()
        );
        assert!(overflow.push(&masked_header(0x80, 1)).is_err());

        let mut exact = Decoder::default();
        assert!(
            exact
                .push(&masked_frame(0x01, &vec![b'x'; MAX_MESSAGE_BYTES - 1],))
                .unwrap()
                .is_empty()
        );
        let final_frame = masked_frame(0x80, b"x");
        let split = final_frame.len() - 1;
        assert!(exact.push(&final_frame[..split]).unwrap().is_empty());
        assert_eq!(
            exact.push(&final_frame[split..]).unwrap(),
            vec![Message::Text("x".repeat(MAX_MESSAGE_BYTES))]
        );

        assert!(Decoder::default().push(&[0xc1, 0x80, 1, 2, 3, 4]).is_err());
        assert!(Decoder::default().push(&[0x09, 0x80, 1, 2, 3, 4]).is_err());
        let mut invalid_utf8 = Decoder::default();
        let frame = masked_frame(0x81, &[0xff]);
        assert!(
            invalid_utf8
                .push(&frame[..frame.len() - 1])
                .unwrap()
                .is_empty()
        );
        assert!(invalid_utf8.push(&frame[frame.len() - 1..]).is_err());
    }

    #[test]
    fn a_text_frame_gives_back_its_payload_at_each_length_form() {
        for length in [0, 125, 126, 65_535, 65_536] {
            let payload = vec![b'x'; length];
            let frame = text_frame(&payload).expect("a frame");
            assert_eq!(
                text_payload(&frame),
                Some(payload.as_slice()),
                "{length} bytes"
            );
        }
        let frame = text_frame(b"{}").expect("a frame");
        assert_eq!(
            text_payload(&frame[..frame.len() - 1]),
            None,
            "a short frame"
        );
        assert_eq!(text_payload(&close_frame()), None, "not a text frame");
    }
}
