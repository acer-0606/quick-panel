#![cfg(windows)]
use quick_panel::{
    model::{ItemKind, ItemSummary, Representation, StoredItem},
    platform::windows::{clipboard, images},
};
fn item(png: &[u8]) -> StoredItem {
    StoredItem {
        summary: ItemSummary {
            id: "fixture".into(),
            kind: ItemKind::Clipboard,
            title: "fixture".into(),
            subtitle: String::new(),
            alias: String::new(),
            tags: vec![],
            pinned: false,
            created_at_ms: 0,
            last_used_ms: 0,
            use_count: 0,
        },
        text: String::new(),
        representations: vec![Representation {
            format: "PNG".into(),
            data: png.to_vec(),
        }],
        target: None,
        open_with: None,
    }
}
#[test]
fn png_restore_prepares_top_down_bgra_dib_without_changing_stored_payload() {
    let item = item(include_bytes!("fixtures/preview-two-pixels.png"));
    let before = item.clone();
    assert!(clipboard::prepare_compatibility(&item, false).is_some());
    let dib = decode_dib(&item.representations[0]);
    assert_eq!(dib.len(), 48);
    assert_eq!(&dib[0..4], &40u32.to_le_bytes());
    assert_eq!(&dib[4..8], &2i32.to_le_bytes());
    assert_eq!(&dib[8..12], &(-1i32).to_le_bytes());
    assert_eq!(&dib[12..16], &[1, 0, 32, 0]);
    assert_eq!(&dib[40..], &[0, 0, 255, 255, 0, 255, 0, 255]);
    assert_eq!(item, before);
    assert!(clipboard::prepare_compatibility(&item, true).is_none());
}
#[test]
fn four_k_png_preview_is_small_bgra_and_keeps_original_dimensions() {
    let image = images::preview(&item(include_bytes!("fixtures/preview-4k.png"))).unwrap();
    assert_eq!((image.source_width, image.source_height), (3840, 2160));
    assert_eq!((image.width, image.height), (1024, 576));
    assert_eq!(image.pixels.len(), 2_359_296);
    assert_eq!(&image.pixels[..4], &[0, 0, 255, 255]);
}
#[test]
fn dib_preview_keeps_memory_source_alive_during_wic_decode() {
    let mut item = item(include_bytes!("fixtures/preview-two-pixels.png"));
    assert!(clipboard::prepare_compatibility(&item, false).is_some());
    let dib = decode_dib(&item.representations[0]);
    item.representations = vec![Representation {
        format: "CF_DIB".into(),
        data: dib,
    }];
    let image = images::preview(&item).unwrap();
    assert_eq!((image.width, image.height), (2, 1));
    assert_eq!(image.pixels, [0, 0, 255, 255, 0, 255, 0, 255]);
    assert!(clipboard::prepare_compatibility(&item, false).is_none());
}

fn decode_dib(rep: &Representation) -> Vec<u8> {
    images::png_dib_writer(rep, |size, fill| {
        let mut bytes = vec![0; size];
        fill(&mut bytes)?;
        Ok(bytes)
    })
    .unwrap()
}
#[test]
fn oversized_compatibility_is_optional_and_original_png_stays_restorable() {
    let item = item(include_bytes!(
        "fixtures/preview-over-compatibility-budget.png"
    ));
    let before = item.clone();
    assert!(images::png_is_readable(&item.representations[0]));
    assert!(clipboard::prepare_compatibility(&item, false).is_none());
    assert_eq!(item, before);
    quick_panel::clipboard_formats::validate_representations(
        &item.representations,
        8 * 1024 * 1024,
    )
    .unwrap();
}
#[test]
fn failed_compatibility_decode_keeps_original_registered_png_payload() {
    let item = item(b"unreadable PNG fixture");
    let before = item.clone();
    assert!(clipboard::prepare_compatibility(&item, false).is_none());
    assert_eq!(item, before);
    quick_panel::clipboard_formats::validate_representations(
        &item.representations,
        8 * 1024 * 1024,
    )
    .unwrap();
}

#[test]
fn screenshot_png_preserves_top_down_pixel_order_and_forces_gdi_alpha_opaque() {
    let png = images::encode_png(
        2,
        2,
        vec![0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0, 1, 255, 255, 255, 17],
    )
    .unwrap();
    assert_eq!(png.format, "PNG");
    assert_eq!(&png.data[..8], b"\x89PNG\r\n\x1a\n");
    let dib = decode_dib(&png);
    assert_eq!(&dib[8..12], &(-2i32).to_le_bytes());
    assert_eq!(
        &dib[40..],
        &[
            0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255, 255, 255, 255, 255
        ]
    );
    assert!(clipboard::prepare_screenshot(png).is_ok());
}

#[test]
fn screenshot_encoder_rejects_malformed_or_over_budget_pixels_before_wic() {
    assert!(images::encode_png(2, 2, vec![0; 15]).is_err());
    assert!(images::encode_png(0, 1, vec![]).is_err());
    assert!(images::encode_png(4097, 4096, vec![]).is_err());
}

#[test]
fn screenshot_encoder_checks_stream_size_before_delivering_large_png() {
    let mut seed = 0x31415926u32;
    let pixels = (0..2048 * 2048)
        .flat_map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            [seed as u8, (seed >> 8) as u8, (seed >> 16) as u8, 0]
        })
        .collect();
    assert!(
        images::encode_png(2048, 2048, pixels)
            .unwrap_err()
            .contains("8 MiB")
    );
}
