#![allow(missing_docs)]

#[test]
fn exact_bytes_have_a_lowercase_fixed_blake3_digest() {
    assert_eq!(
        dudes_journal::mail_body_digest(b""),
        "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
    );
    let input = b"mail\0with\ncontrols";
    let before = input.to_vec();
    let digest = dudes_journal::mail_body_digest(input);
    assert_eq!(digest.len(), 64);
    assert!(
        digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );
    assert_ne!(
        digest,
        dudes_journal::mail_body_digest(b"mail\0withcontrols")
    );
    assert_eq!(input.as_slice(), before.as_slice());
}

#[test]
fn recipient_receipt_uses_canonical_byte_order_and_first_eighty_digest_bits() {
    let outbox =
        dudes_types::MailId::from_bytes(core::array::from_fn(|n| u8::try_from(n).unwrap()));
    let recipient =
        dudes_types::AgentId::from_bytes(core::array::from_fn(|n| u8::try_from(n + 16).unwrap()));
    let receipt = dudes_journal::mail_recipient_id(outbox, recipient);
    assert_eq!(
        receipt.to_bytes(),
        [
            0, 1, 2, 3, 4, 5, 0xe5, 0x28, 0xe9, 0x57, 0x98, 3, 0x7d, 0xf4, 0x10, 0x54
        ]
    );
    assert_eq!(receipt.timestamp_ms(), 0x0001_0203_0405);
}
