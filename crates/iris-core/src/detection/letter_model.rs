use std::sync::OnceLock;

const EMBEDDED_MODEL_BYTES: &[u8] = include_bytes!("../../../../assets/letters-model.bin");

#[derive(Debug, Clone)]
struct Layer {
    rows: usize,
    cols: usize,
    weights: Vec<f32>,
    biases: Vec<f32>,
}

/// Pure-Rust 2-layer MLP glyph classifier loading `letters-model.bin` (`IRISLM01`, v1).
#[derive(Debug, Clone)]
pub struct LetterModel {
    charset: Vec<char>,
    charset_str: String,
    input_size: usize,
    layers: Vec<Layer>,
    max_width: usize,
}

impl LetterModel {
    pub const FORMAT_VERSION: u32 = 1;
    pub const MIN_PROBABILITY: f64 = 1e-6;
    const MAGIC: &'static [u8; 8] = b"IRISLM01";

    pub fn embedded() -> &'static LetterModel {
        static INSTANCE: OnceLock<LetterModel> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            Self::from_bytes(EMBEDDED_MODEL_BYTES)
                .expect("Embedded assets/letters-model.bin must be valid IRISLM01")
        })
    }

    pub fn from_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
        let mut pos = 0usize;
        let take = |pos: &mut usize, n: usize| -> anyhow::Result<&[u8]> {
            if *pos + n > bytes.len() {
                anyhow::bail!("Unexpected end of letters-model.bin");
            }
            let s = &bytes[*pos..*pos + n];
            *pos += n;
            Ok(s)
        };
        let read_u32 = |pos: &mut usize| -> anyhow::Result<u32> {
            let s = take(pos, 4)?;
            Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        };
        let read_f32 = |pos: &mut usize| -> anyhow::Result<f32> {
            let s = take(pos, 4)?;
            Ok(f32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        };

        let magic = take(&mut pos, 8)?;
        if magic != Self::MAGIC {
            anyhow::bail!("Not an IRIS letter model (bad magic).");
        }
        let version = read_u32(&mut pos)?;
        if version != Self::FORMAT_VERSION {
            anyhow::bail!("Unsupported letter model version {}.", version);
        }
        let charset_len = read_u32(&mut pos)? as usize;
        if charset_len > 4096 {
            anyhow::bail!("Charset is implausibly long.");
        }
        let charset_bytes = take(&mut pos, charset_len)?;
        let charset_str = std::str::from_utf8(charset_bytes)?.to_string();
        let charset: Vec<char> = charset_str.chars().collect();

        let input_size = read_u32(&mut pos)? as usize;
        let layer_count = read_u32(&mut pos)? as usize;
        if input_size == 0 || layer_count == 0 || layer_count > 16 {
            anyhow::bail!("Bad model header.");
        }

        let mut layers = Vec::with_capacity(layer_count);
        let mut expected_rows = input_size;
        let mut max_width = 0usize;

        for l in 0..layer_count {
            let rows = read_u32(&mut pos)? as usize;
            let cols = read_u32(&mut pos)? as usize;
            if rows != expected_rows || cols == 0 || rows.saturating_mul(cols) > 50_000_000 {
                anyhow::bail!("Layer {} has unexpected shape {}x{}.", l, rows, cols);
            }
            let mut weights = Vec::with_capacity(rows * cols);
            for _ in 0..rows * cols {
                weights.push(read_f32(&mut pos)?);
            }
            let mut biases = Vec::with_capacity(cols);
            for _ in 0..cols {
                biases.push(read_f32(&mut pos)?);
            }
            max_width = max_width.max(cols);
            layers.push(Layer {
                rows,
                cols,
                weights,
                biases,
            });
            expected_rows = cols;
        }

        if expected_rows != charset.len() {
            anyhow::bail!("Output size does not match charset length.");
        }
        if pos != bytes.len() {
            anyhow::bail!("Unexpected trailing bytes after last layer.");
        }

        Ok(Self {
            charset,
            charset_str,
            input_size,
            layers,
            max_width,
        })
    }

    pub fn charset_str(&self) -> &str {
        &self.charset_str
    }

    pub fn charset(&self) -> &[char] {
        &self.charset
    }

    pub fn input_size(&self) -> usize {
        self.input_size
    }

    pub fn class_count(&self) -> usize {
        self.charset.len()
    }

    pub fn scratch_size(&self) -> usize {
        2 * self.max_width
    }

    pub fn predict_log_probs(&self, input: &[f64], log_probs: &mut [f64]) {
        let mut scratch = vec![0.0f64; self.scratch_size()];
        self.predict_log_probs_with_scratch(input, log_probs, &mut scratch);
    }

    pub fn predict_log_probs_with_scratch(
        &self,
        input: &[f64],
        log_probs: &mut [f64],
        scratch: &mut [f64],
    ) {
        assert_eq!(input.len(), self.input_size);
        assert!(log_probs.len() >= self.class_count());
        assert!(scratch.len() >= self.scratch_size());

        let (buf_a, buf_b) = scratch.split_at_mut(self.max_width);

        for (l_idx, layer) in self.layers.iter().enumerate() {
            let cols = layer.cols;
            if l_idx == 0 {
                let next = &mut buf_a[..cols];
                for j in 0..cols {
                    next[j] = layer.biases[j] as f64;
                }
                for i in 0..layer.rows {
                    let x = input[i];
                    if x == 0.0 {
                        continue;
                    }
                    let w_row = &layer.weights[i * cols..(i + 1) * cols];
                    for j in 0..cols {
                        next[j] += x * (w_row[j] as f64);
                    }
                }
                if l_idx + 1 < self.layers.len() {
                    for v in next.iter_mut() {
                        if *v < 0.0 {
                            *v = 0.0;
                        }
                    }
                } else {
                    finalize_log_softmax(next, log_probs);
                }
            } else {
                let (cur, next_full) = if l_idx % 2 == 1 {
                    (&buf_a[..layer.rows], &mut buf_b[..cols])
                } else {
                    (&buf_b[..layer.rows], &mut buf_a[..cols])
                };
                for j in 0..cols {
                    next_full[j] = layer.biases[j] as f64;
                }
                for i in 0..layer.rows {
                    let x = cur[i];
                    if x == 0.0 {
                        continue;
                    }
                    let w_row = &layer.weights[i * cols..(i + 1) * cols];
                    for j in 0..cols {
                        next_full[j] += x * (w_row[j] as f64);
                    }
                }
                if l_idx + 1 < self.layers.len() {
                    for v in next_full.iter_mut() {
                        if *v < 0.0 {
                            *v = 0.0;
                        }
                    }
                } else {
                    finalize_log_softmax(next_full, log_probs);
                }
            }
        }
    }
}

fn finalize_log_softmax(logits: &mut [f64], log_probs: &mut [f64]) {
    let mut max_val = logits[0];
    for &v in &logits[1..] {
        if v > max_val {
            max_val = v;
        }
    }
    let mut sum = 0.0f64;
    for v in logits.iter_mut() {
        *v = (*v - max_val).exp();
        sum += *v;
    }
    for (j, &v) in logits.iter().enumerate() {
        log_probs[j] = (v / sum).clamp(LetterModel::MIN_PROBABILITY, 1.0).ln();
    }
}

