//! The frame codec, the torn-tail scanner and the dropped-region frame search must never panic
//! on any bytes, and a frame that decodes must re-encode to exactly the bytes it came from
//! (the journal frame format).

#![no_main]

use dudes_journal::reader::FrameSearch;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(decoded) = dudes_journal::frame::decode(data) {
        let mut again = Vec::new();
        dudes_journal::frame::encode(decoded.kind, decoded.payload, &mut again)
            .expect("a decoded payload fits");
        assert_eq!(again.len(), decoded.total_len);
        assert_eq!(&again[..], &data[..decoded.total_len]);
    }
    let outcome = dudes_journal::reader::scan(data);
    assert!(outcome.good_end <= data.len() as u64);
    if let Some(failure) = outcome.failure {
        assert_eq!(failure.offset, outcome.good_end);
    } else {
        assert_eq!(outcome.good_end, data.len() as u64);
    }
    match dudes_journal::reader::find_frame(data) {
        FrameSearch::Found(at) => {
            assert!(at <= data.len());
            assert!(dudes_journal::frame::decode(&data[at..]).is_ok());
        }
        FrameSearch::Absent | FrameSearch::OverBudget => {}
    }
});
