use fframes_studio::frame_image::{
    ConversionError, convert_rgba_to_gpui_bgra, create_render_image, generate_reference_frame,
};
use fframes_studio_protocol::{
    AlphaMode, CURRENT_PROTOCOL_VERSION, ChannelOrder, ColorSpace, FrameHeader, ProtocolError,
};

#[test]
fn test_stride_removal_and_padding_strip() {
    let width = 64;
    let height = 32;
    let (header, payload) = generate_reference_frame("rev_test", 1, 1, 0, width, height, true);
    assert!(header.stride_bytes > width * 4);

    let bgra = convert_rgba_to_gpui_bgra(&header, &payload).expect("should convert with padding");
    assert_eq!(bgra.len(), (width * height * 4) as usize);

    // Verify RenderImage can be created without a display
    let render_image = create_render_image(&header, &payload).expect("render image created");
    assert_eq!(render_image.frame_count(), 1);
}

#[test]
fn test_channel_order_swapping() {
    let width = 2;
    let height = 2;
    let stride_bytes = 8;
    let mut payload = vec![0u8; 16];

    // Pixel 0: Red [255, 0, 0, 255]
    payload[0] = 255;
    payload[1] = 0;
    payload[2] = 0;
    payload[3] = 255;

    // Pixel 1: Blue [0, 0, 255, 255]
    payload[4] = 0;
    payload[5] = 0;
    payload[6] = 255;
    payload[7] = 255;

    let header = FrameHeader {
        protocol_version: CURRENT_PROTOCOL_VERSION,
        source_revision: "rev1".into(),
        worker_generation: 1,
        request_id: 1,
        frame_index: 0,
        width,
        height,
        stride_bytes,
        channel_order: ChannelOrder::Rgba8,
        alpha_mode: AlphaMode::Straight,
        color_space: ColorSpace::Srgb,
        payload_len: 16,
    };

    let bgra = convert_rgba_to_gpui_bgra(&header, &payload).expect("converts RGBA to BGRA");

    // Pixel 0: In BGRA, Blue is at index 0, Red is at index 2
    assert_eq!(bgra[0], 0); // B
    assert_eq!(bgra[1], 0); // G
    assert_eq!(bgra[2], 255); // R
    assert_eq!(bgra[3], 255); // A

    // Pixel 1: Blue [0, 0, 255, 255] in RGBA -> B=255, G=0, R=0, A=255 in BGRA
    assert_eq!(bgra[4], 255); // B
    assert_eq!(bgra[5], 0); // G
    assert_eq!(bgra[6], 0); // R
    assert_eq!(bgra[7], 255); // A
}

#[test]
fn test_premultiplied_alpha_unpremultiplication() {
    let width = 1;
    let height = 1;
    let stride_bytes = 4;
    // RGBA with premultiplied alpha: R=128, G=64, B=32, A=128 (alpha = 0.5)
    // Expected un-premultiplied: R ≈ 255, G ≈ 128, B ≈ 64
    // And converted to BGRA: B ≈ 64, G ≈ 128, R ≈ 255
    let payload = vec![128, 64, 32, 128];

    let header = FrameHeader {
        protocol_version: CURRENT_PROTOCOL_VERSION,
        source_revision: "rev1".into(),
        worker_generation: 1,
        request_id: 1,
        frame_index: 0,
        width,
        height,
        stride_bytes,
        channel_order: ChannelOrder::Rgba8,
        alpha_mode: AlphaMode::Premultiplied,
        color_space: ColorSpace::Srgb,
        payload_len: 4,
    };

    let bgra = convert_rgba_to_gpui_bgra(&header, &payload).expect("un-premultiplies");
    // B ≈ 64
    assert!((bgra[0] as i32 - 64).abs() <= 1);
    // G ≈ 128
    assert!((bgra[1] as i32 - 128).abs() <= 1);
    // R ≈ 255
    assert!((bgra[2] as i32 - 255).abs() <= 1);
    // A = 128
    assert_eq!(bgra[3], 128);
}

#[test]
fn test_invalid_buffer_and_header_rejections() {
    // 1. Payload shorter than header.payload_len
    let (header, mut payload) = generate_reference_frame("rev1", 1, 1, 0, 10, 10, false);
    payload.truncate(header.payload_len - 5);
    assert!(matches!(
        convert_rgba_to_gpui_bgra(&header, &payload),
        Err(ConversionError::PayloadTooShort { .. })
    ));

    // 2. Zero dimensions
    let invalid_zero = FrameHeader {
        protocol_version: CURRENT_PROTOCOL_VERSION,
        source_revision: "rev".into(),
        worker_generation: 1,
        request_id: 1,
        frame_index: 0,
        width: 0,
        height: 10,
        stride_bytes: 0,
        channel_order: ChannelOrder::Rgba8,
        alpha_mode: AlphaMode::Straight,
        color_space: ColorSpace::Srgb,
        payload_len: 0,
    };
    assert!(matches!(
        convert_rgba_to_gpui_bgra(&invalid_zero, &[]),
        Err(ConversionError::Protocol(
            ProtocolError::ZeroDimensions { .. }
        ))
    ));

    // 3. Stride smaller than width * 4
    let invalid_stride = FrameHeader {
        protocol_version: CURRENT_PROTOCOL_VERSION,
        source_revision: "rev".into(),
        worker_generation: 1,
        request_id: 1,
        frame_index: 0,
        width: 10,
        height: 10,
        stride_bytes: 35, // needs >= 40
        channel_order: ChannelOrder::Rgba8,
        alpha_mode: AlphaMode::Straight,
        color_space: ColorSpace::Srgb,
        payload_len: 350,
    };
    assert!(matches!(
        convert_rgba_to_gpui_bgra(&invalid_stride, &[0; 350]),
        Err(ConversionError::Protocol(
            ProtocolError::StrideTooSmall { .. }
        ))
    ));

    // 4. Exceeds max payload cap (64 MiB)
    let huge_header = FrameHeader {
        protocol_version: CURRENT_PROTOCOL_VERSION,
        source_revision: "rev".into(),
        worker_generation: 1,
        request_id: 1,
        frame_index: 0,
        width: 8000,
        height: 3000,
        stride_bytes: 32000,
        channel_order: ChannelOrder::Rgba8,
        alpha_mode: AlphaMode::Straight,
        color_space: ColorSpace::Srgb,
        payload_len: 32000 * 3000, // 96 MB > 64 MB
    };
    assert!(matches!(
        convert_rgba_to_gpui_bgra(&huge_header, &[0; 10]),
        Err(ConversionError::Protocol(
            ProtocolError::PayloadExceedsCap { .. }
        ))
    ));
}
