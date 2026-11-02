use anyhow::{Result, bail};

const MAX_EVENT_BYTES: usize = 262_144;

/// Frame SSE bytes before decoding UTF-8: HTTP chunks can split any character or line.
#[derive(Default)]
pub(crate) struct SseDecoder {
    pending: Vec<u8>,
    data: String,
}

impl SseDecoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>> {
        self.pending.extend_from_slice(bytes);
        let mut events = Vec::new();
        let mut consumed = 0;
        while let Some(length) = self.pending[consumed..]
            .iter()
            .position(|byte| *byte == b'\n')
        {
            let end = consumed + length;
            if length > MAX_EVENT_BYTES {
                bail!("SSE line exceeded 256 KiB.");
            }
            let line = std::str::from_utf8(&self.pending[consumed..end])
                .map_err(|_| anyhow::anyhow!("SSE returned invalid UTF-8."))?
                .trim_end_matches('\r');
            if line.is_empty() {
                if !self.data.is_empty() {
                    self.data.pop(); // SSE joins data lines with one newline, excluding the last.
                    events.push(std::mem::take(&mut self.data));
                }
            } else if let Some(value) = line.strip_prefix("data:") {
                let value = value.strip_prefix(' ').unwrap_or(value);
                if self.data.len() + value.len() + 1 > MAX_EVENT_BYTES {
                    bail!("SSE event exceeded 256 KiB.");
                }
                self.data.push_str(value);
                self.data.push('\n');
            }
            consumed = end + 1;
        }
        self.pending.drain(..consumed);
        if self.pending.len() > MAX_EVENT_BYTES {
            bail!("SSE line exceeded 256 KiB.");
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragmentation_preserves_unicode_crlf_comments_and_multiline_data() {
        let bytes = ": keepalive\r\nevent: ignored\r\ndata: {\"text\":\r\ndata: \"界🙂\"}\r\n\r\ndata: [DONE]\n\n".as_bytes();
        for size in 1..=bytes.len() {
            let mut decoder = SseDecoder::default();
            let mut events = Vec::new();
            for chunk in bytes.chunks(size) {
                events.extend(decoder.push(chunk).unwrap());
            }
            assert_eq!(events, vec!["{\"text\":\n\"界🙂\"}", "[DONE]"]);
        }
    }

    #[test]
    fn unfinished_and_oversized_events_cannot_become_completions() {
        let mut decoder = SseDecoder::default();
        assert!(decoder.push(b"data: [DONE]\n").unwrap().is_empty());
        assert!(decoder.push(&vec![b'x'; MAX_EVENT_BYTES + 1]).is_err());
        assert!(SseDecoder::default().push(b"data: \xff\n\n").is_err());
    }
}
