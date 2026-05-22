//! Content chunking for the Memory Manager.
//!
//! Splits Markdown content into overlapping line-based chunks. Each chunk
//! tracks its 1-indexed start and end line numbers in the original file.

/// A raw text chunk before embedding.
#[derive(Debug, Clone)]
pub struct TextChunk {
    pub text: String,
    /// Start line in the original file (1-indexed).
    pub start_line: u32,
    /// End line in the original file (1-indexed).
    pub end_line: u32,
}

/// Split `content` into overlapping chunks of `chunk_size` lines with `chunk_overlap` lines of
/// overlap between consecutive chunks.
///
/// Returns an empty vec if `content` is empty or whitespace-only.
pub fn chunk_text(content: &str, chunk_size: usize, chunk_overlap: usize) -> Vec<TextChunk> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() || lines.iter().all(|l| l.trim().is_empty()) {
        return Vec::new();
    }

    let total = lines.len();

    if total <= chunk_size {
        return vec![TextChunk {
            text: content.to_string(),
            start_line: 1,
            end_line: total as u32,
        }];
    }

    let step = if chunk_size > chunk_overlap {
        chunk_size - chunk_overlap
    } else {
        1
    };

    let mut chunks = Vec::new();
    let mut start = 0usize;

    while start < total {
        let end = (start + chunk_size).min(total);
        let text = lines[start..end].join("\n");
        chunks.push(TextChunk {
            text,
            start_line: (start + 1) as u32,
            end_line: end as u32,
        });

        if end == total {
            break;
        }
        start += step;
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_content_produces_no_chunks() {
        let chunks = chunk_text("", 400, 80);
        assert!(chunks.is_empty());
    }

    #[test]
    fn whitespace_only_produces_no_chunks() {
        let chunks = chunk_text("   \n\n  \n", 400, 80);
        assert!(chunks.is_empty());
    }

    #[test]
    fn short_content_produces_single_chunk() {
        let content = "Hello world.\nThis is a test.\nThird line.";
        let chunks = chunk_text(content, 400, 80);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].start_line, 1);
        assert_eq!(chunks[0].end_line, 3);
        assert_eq!(chunks[0].text, content);
    }

    #[test]
    fn long_content_produces_overlapping_chunks() {
        let lines: Vec<String> = (1..=100)
            .map(|i| format!("Line number {} has some words in it.", i))
            .collect();
        let content = lines.join("\n");
        let chunks = chunk_text(&content, 20, 5);

        let ranges: Vec<(u32, u32)> = chunks.iter().map(|c| (c.start_line, c.end_line)).collect();
        assert_eq!(
            ranges,
            vec![
                (1, 20),
                (16, 35),
                (31, 50),
                (46, 65),
                (61, 80),
                (76, 95),
                (91, 100),
            ]
        );
    }

    #[test]
    fn chunk_text_preserves_all_content() {
        let lines: Vec<String> = (1..=50).map(|i| format!("Line {}", i)).collect();
        let content = lines.join("\n");
        let chunks = chunk_text(&content, 10, 3);

        for line in &lines {
            assert!(
                chunks.iter().any(|c| c.text.contains(line.as_str())),
                "Line '{}' not found in any chunk",
                line
            );
        }
    }
}
