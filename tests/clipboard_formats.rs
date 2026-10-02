use quick_panel::clipboard_formats::{
    decode_hdrop, decode_unicode_text, encode_unicode_text, validate_representations,
};
use quick_panel::model::Representation;

fn wide(value: &str) -> Vec<u8> {
    value.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn hdrop(payload: &[u8], offset: u32, unicode: bool) -> Vec<u8> {
    let mut buffer = vec![0; 20];
    buffer[0..4].copy_from_slice(&offset.to_le_bytes());
    buffer[16..20].copy_from_slice(&u32::from(unicode).to_le_bytes());
    buffer.extend_from_slice(payload);
    buffer
}

#[test]
fn unicode_text_preserves_unicode_and_stops_at_terminator() {
    assert_eq!(
        decode_unicode_text(&wide("你好🦀\0padding")).unwrap(),
        "你好🦀"
    );
    assert_eq!(encode_unicode_text("你好"), wide("你好\0"));
}

#[test]
fn malformed_unicode_text_is_rejected() {
    for buffer in [vec![0], wide("unterminated"), vec![0x00, 0xd8, 0, 0]] {
        assert!(decode_unicode_text(&buffer).is_err());
    }
}

#[test]
fn hdrop_reads_multiple_unicode_paths() {
    let buffer = hdrop(&wide("C:\\文档\\一.txt\0D:\\🙂\\二.pdf\0\0"), 20, true);
    assert_eq!(
        decode_hdrop(&buffer).unwrap(),
        vec!["C:\\文档\\一.txt", "D:\\🙂\\二.pdf"]
    );
}

#[test]
fn hdrop_rejects_invalid_offsets_truncation_and_ansi() {
    for buffer in [
        vec![0; 19],
        hdrop(&wide("a\0\0"), 19, true),
        hdrop(&wide("a\0\0"), 99, true),
        hdrop(&wide("a\0"), 20, true),
        hdrop(&[1, 0, 0], 20, true),
        hdrop(b"a\0\0", 20, false),
    ] {
        assert!(decode_hdrop(&buffer).is_err());
    }
}

#[test]
fn representation_budget_counts_all_formats_and_rejects_private_handles() {
    let reps = vec![
        Representation {
            format: "PNG".into(),
            data: vec![1; 4],
        },
        Representation {
            format: "CF_DIB".into(),
            data: vec![1; 5],
        },
    ];
    assert!(validate_representations(&reps, 8).is_err());
    assert!(validate_representations(&reps, 9).is_ok());
    assert!(
        validate_representations(
            &[Representation {
                format: "PrivatePointer".into(),
                data: vec![1]
            }],
            8
        )
        .is_err()
    );
    assert!(
        validate_representations(
            &[Representation {
                format: "CF_UNICODETEXT".into(),
                data: vec![1]
            }],
            8
        )
        .is_err()
    );
}

#[test]
fn parser_and_restore_never_accept_payload_above_hard_cap() {
    let oversized = vec![0; 8 * 1024 * 1024 + 2];
    assert!(decode_unicode_text(&oversized).is_err());
    assert!(decode_hdrop(&hdrop(&oversized, 20, true)).is_err());
    assert!(
        validate_representations(
            &[Representation {
                format: "PNG".into(),
                data: oversized
            }],
            usize::MAX
        )
        .is_err()
    );
}

#[test]
fn duplicate_formats_are_rejected_before_clipboard_mutation() {
    let representations = vec![
        Representation {
            format: "PNG".into(),
            data: vec![1],
        },
        Representation {
            format: "PNG".into(),
            data: vec![2],
        },
    ];
    assert!(validate_representations(&representations, 8).is_err());
}

#[test]
fn ansi_hdrop_is_preserved_but_validated_before_windows_decoding() {
    let valid = Representation {
        format: "CF_HDROP".into(),
        data: hdrop(b"C:\\a.txt\0D:\\b.txt\0\0", 20, false),
    };
    assert!(validate_representations(&[valid], 1024).is_ok());
    let truncated = Representation {
        format: "CF_HDROP".into(),
        data: hdrop(b"C:\\a.txt\0", 20, false),
    };
    assert!(validate_representations(&[truncated], 1024).is_err());
}
