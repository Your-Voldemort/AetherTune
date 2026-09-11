//! Parsing for mpv's JSON IPC line protocol.
//!
//! This is pure string-in/data-out logic with no dependency on how the
//! lines arrived — player.rs feeds it lines read off a Unix socket on
//! Linux/macOS or off a named-pipe reader thread on Windows, and both
//! paths call the same functions here.

pub struct StreamInfo {
    /// Actual audio bitrate in bits/sec from mpv (0 if unknown)
    pub audio_bitrate: f64,
    /// Audio codec name reported by mpv
    pub audio_codec: String,
    /// Demuxer cache duration in seconds (how much audio is buffered)
    pub cache_duration: f64,
    /// How long the current stream has been connected
    pub stream_connected_at: Option<std::time::Instant>,
    /// Audio sample rate from mpv
    pub sample_rate: u32,
    /// Audio channel count
    pub channels: u32,
}

impl StreamInfo {
    pub fn new() -> Self {
        Self {
            audio_bitrate: 0.0,
            audio_codec: String::new(),
            cache_duration: 0.0,
            stream_connected_at: None,
            sample_rate: 0,
            channels: 0,
        }
    }

    pub fn reset(&mut self) {
        self.audio_bitrate = 0.0;
        self.audio_codec.clear();
        self.cache_duration = 0.0;
        self.stream_connected_at = None;
        self.sample_rate = 0;
        self.channels = 0;
    }

    pub fn uptime_str(&self) -> String {
        match self.stream_connected_at {
            Some(t) => {
                let secs = t.elapsed().as_secs();
                if secs >= 3600 {
                    format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
                } else if secs >= 60 {
                    format!("{}m {}s", secs / 60, secs % 60)
                } else {
                    format!("{}s", secs)
                }
            }
            None => "—".to_string(),
        }
    }
}

/// Update `info` from one line of mpv IPC JSON, if it's a reply this
/// module recognizes (request_id 200/201, or observe_property ids 2-4).
pub fn parse_stream_info(info: &mut StreamInfo, text: &str) {
    if text.contains("\"request_id\":200") || text.contains("\"request_id\": 200") {
        if let Some(val) = extract_number(text) {
            info.audio_bitrate = val;
        }
    }
    if text.contains("\"request_id\":201") || text.contains("\"request_id\": 201") {
        if let Some(val) = extract_number(text) {
            info.cache_duration = val;
        }
    }
    if text.contains("\"id\":2") || text.contains("\"id\": 2") {
        if let Some(val) = extract_string_value(text) {
            info.audio_codec = val;
        }
    }
    if text.contains("\"id\":3") || text.contains("\"id\": 3") {
        if let Some(val) = extract_number(text) {
            info.sample_rate = val as u32;
        }
    }
    if text.contains("\"id\":4") || text.contains("\"id\": 4") {
        if let Some(val) = extract_number(text) {
            info.channels = val as u32;
        }
    }
}

pub fn extract_number(json: &str) -> Option<f64> {
    let data_key = "\"data\":";
    let idx = json.find(data_key)?;
    let after = json[idx + data_key.len()..].trim_start();
    let num_str: String = after
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    num_str.parse::<f64>().ok()
}

pub fn extract_string_value(json: &str) -> Option<String> {
    let data_key = "\"data\":";
    let idx = json.find(data_key)?;
    let after = json[idx + data_key.len()..].trim_start();
    if after.starts_with('"') {
        let rest = &after[1..];
        let end = rest.find('"')?;
        Some(rest[..end].to_string())
    } else {
        None
    }
}

pub fn extract_media_title(json_line: &str) -> Option<String> {
    if !json_line.contains("media-title") {
        return None;
    }

    let data_key = "\"data\":";
    let idx = json_line.find(data_key)?;
    let after = &json_line[idx + data_key.len()..];
    let trimmed = after.trim_start();

    if trimmed.starts_with('"') {
        let rest = &trimmed[1..];
        let mut result = String::new();
        let mut chars = rest.chars();
        while let Some(ch) = chars.next() {
            match ch {
                '"' => return Some(result),
                '\\' => {
                    if let Some(escaped) = chars.next() {
                        match escaped {
                            '"' => result.push('"'),
                            '\\' => result.push('\\'),
                            'n' => result.push(' '),
                            _ => result.push(escaped),
                        }
                    }
                }
                _ => result.push(ch),
            }
        }
    }
    None
}