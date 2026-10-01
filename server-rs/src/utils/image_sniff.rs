// 图片魔数嗅探与扩展名映射(单点实现)。
//
// 抽取背景(2026-10-02 视觉能力包 D2):图像通道的落盘校验、`/api/images/{file}` 路由
// 与历史引用解析都需要「按真实字节判断图片格式」,而头像路由此前有一份同构私有实现。
// 为避免同一种字节序列在多处各自解释,把完整版(含 BMP)收敛到这里,头像路由改引用
// 本实现(行为不变);`parsing/character_card.rs` 的副本服务于卡片内嵌数据的窄场景,
// 暂不动(第三份的产生原因在 D2 评审中已登记,后续批次按需收敛)。
pub fn sniff_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(b"BM") {
        Some("image/bmp")
    } else {
        None
    }
}

/// 图像 mime → 文件扩展名(落盘文件名用;仅覆盖上传白名单的四种)
pub fn extension_for_mime(mime: &str) -> Option<&'static str> {
    match mime {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_covers_whitelist_and_bmp() {
        assert_eq!(
            sniff_image_mime(&[0x89, 0x50, 0x4E, 0x47, 0x0D]),
            Some("image/png")
        );
        assert_eq!(
            sniff_image_mime(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some("image/jpeg")
        );
        assert_eq!(sniff_image_mime(b"GIF89a"), Some("image/gif"));
        assert_eq!(
            sniff_image_mime(b"RIFF\x00\x00\x00\x00WEBPVP8 "),
            Some("image/webp")
        );
        assert_eq!(sniff_image_mime(b"BM\x00\x00"), Some("image/bmp"));
        assert_eq!(sniff_image_mime(b"not an image"), None);
        assert_eq!(sniff_image_mime(&[]), None);
    }

    #[test]
    fn extension_mapping_only_whitelist() {
        assert_eq!(extension_for_mime("image/png"), Some("png"));
        assert_eq!(extension_for_mime("image/jpeg"), Some("jpg"));
        assert_eq!(extension_for_mime("image/gif"), Some("gif"));
        assert_eq!(extension_for_mime("image/webp"), Some("webp"));
        // 非白名单与嗅探可得但不上传的 bmp 一律无扩展名
        assert_eq!(extension_for_mime("image/bmp"), None);
        assert_eq!(extension_for_mime("text/plain"), None);
    }
}
