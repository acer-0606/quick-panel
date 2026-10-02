use crate::model::Representation;

pub const MAX_ITEM_BYTES: usize = 8 * 1024 * 1024;
pub const SUPPORTED_FORMATS: &[&str] = &[
    "CF_UNICODETEXT",
    "CF_DIB",
    "CF_DIBV5",
    "CF_HDROP",
    "PNG",
    "HTML Format",
    "Rich Text Format",
];

fn check_utf16(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > MAX_ITEM_BYTES {
        return Err("UTF-16数据超过大小上限".into());
    }
    if !bytes.len().is_multiple_of(2) {
        return Err("UTF-16数据长度不完整".into());
    }
    Ok(())
}

fn decode_utf16_slice(bytes: &[u8]) -> Result<String, String> {
    let units = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
    let mut text = String::new();
    // Every UTF-16 unit needs at most three UTF-8 bytes; bound capacity before decoding.
    let capacity = (bytes.len() / 2).saturating_mul(3).min(MAX_ITEM_BYTES);
    text.try_reserve_exact(capacity)
        .map_err(|_| "无法分配文本内存")?;
    for character in char::decode_utf16(units) {
        let character = character.map_err(|_| "文本含无效UTF-16")?;
        if text.len().saturating_add(character.len_utf8()) > MAX_ITEM_BYTES {
            return Err("解码文本超过大小上限".into());
        }
        text.push(character);
    }
    Ok(text)
}

pub fn decode_unicode_text(bytes: &[u8]) -> Result<String, String> {
    check_utf16(bytes)?;
    let end = bytes
        .chunks_exact(2)
        .position(|pair| pair == [0, 0])
        .ok_or("文本缺少终止符")?
        * 2;
    decode_utf16_slice(&bytes[..end])
}

pub fn encode_unicode_text(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn hdrop_payload(bytes: &[u8]) -> Result<(&[u8], bool), String> {
    if bytes.len() > MAX_ITEM_BYTES {
        return Err("文件列表超过大小上限".into());
    }
    if bytes.len() < 20 {
        return Err("DROPFILES头不完整".into());
    }
    let offset = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
    let unicode = u32::from_le_bytes(bytes[16..20].try_into().unwrap()) != 0;
    if offset < 20 || offset >= bytes.len() || (unicode && !offset.is_multiple_of(2)) {
        return Err("DROPFILES文件偏移无效".into());
    }
    Ok((&bytes[offset..], unicode))
}

/// Unicode DROPFILES parser. Legacy ANSI lists require the originating Windows codepage.
pub fn decode_hdrop(bytes: &[u8]) -> Result<Vec<String>, String> {
    let (payload, unicode) = hdrop_payload(bytes)?;
    if !unicode {
        return Err("ANSI文件列表须由Windows解码".into());
    }
    check_utf16(payload)?;
    let mut paths = Vec::new();
    let mut start = 0;
    let mut allocation_bytes = 0usize;
    loop {
        let end = payload[start..]
            .chunks_exact(2)
            .position(|pair| pair == [0, 0])
            .map(|i| i * 2 + start)
            .ok_or("文件列表缺少双终止符")?;
        if end == start {
            if paths.is_empty() && payload.get(end + 2..end + 4) != Some(&[0, 0]) {
                return Err("空文件列表缺少双终止符".into());
            }
            return Ok(paths);
        }
        let capacity = ((end - start) / 2).saturating_mul(3);
        allocation_bytes = allocation_bytes
            .saturating_add(capacity)
            .saturating_add(std::mem::size_of::<String>());
        if allocation_bytes > MAX_ITEM_BYTES {
            return Err("文件列表解码内存超过大小上限".into());
        }
        paths.push(decode_utf16_slice(&payload[start..end])?);
        start = end + 2;
    }
}

pub fn validate_representations(
    representations: &[Representation],
    limit: usize,
) -> Result<(), String> {
    let mut total = 0usize;
    for (index, representation) in representations.iter().enumerate() {
        if !SUPPORTED_FORMATS.contains(&representation.format.as_str()) {
            return Err("不支持恢复该剪贴板格式".into());
        }
        if representation.data.is_empty() {
            return Err("剪贴板格式为空".into());
        }
        if representations[..index]
            .iter()
            .any(|other| other.format == representation.format)
        {
            return Err("剪贴板格式重复".into());
        }
        total = total
            .checked_add(representation.data.len())
            .ok_or("剪贴板大小溢出")?;
        if total > limit.min(MAX_ITEM_BYTES) {
            return Err("剪贴板数据超过大小上限".into());
        }
        match representation.format.as_str() {
            "CF_UNICODETEXT" => {
                decode_unicode_text(&representation.data)?;
            }
            "CF_HDROP" => {
                let (payload, unicode) = hdrop_payload(&representation.data)?;
                if unicode {
                    decode_hdrop(&representation.data)?;
                } else if !payload.windows(2).any(|pair| pair == [0, 0]) {
                    return Err("ANSI文件列表缺少双终止符".into());
                }
            }
            _ => {}
        }
    }
    Ok(())
}
