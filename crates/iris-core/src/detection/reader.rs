use crate::detection::detector::NameplateDetectorSettings;
use crate::detection::features::{CapBand, LineMask, NameplateFeatures};
use crate::detection::image_ops::BgrImage;
use crate::detection::letter_model::LetterModel;
use crate::models::IntRect;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NameplateRead {
    pub text: String,
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReadPiece {
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub score: f64,
    pub free_score: f64,
}

#[derive(Debug, Clone)]
pub struct PieceRead {
    pub pieces: Vec<ReadPiece>,
    pub cap_height: usize,
    pub character_spans: Vec<(usize, usize)>,
    pub mask: Option<LineMask>,
}

#[derive(Debug, Clone)]
pub struct NameplateReader {
    model: &'static LetterModel,
}

impl Default for NameplateReader {
    fn default() -> Self {
        Self::new(LetterModel::embedded())
    }
}

impl NameplateReader {
    const SPLIT_FAILURE_SCORE: f64 = -50.0;

    pub fn new(model: &'static LetterModel) -> Self {
        assert_eq!(
            model.input_size(),
            NameplateFeatures::VECTOR_LENGTH,
            "Model input size mismatch"
        );
        Self { model }
    }

    pub fn model(&self) -> &'static LetterModel {
        self.model
    }

    pub fn create_alphabet(&self, characters: &str) -> Vec<bool> {
        let mut allowed = vec![false; self.model.class_count()];
        let mut any = false;
        for (i, &ch) in self.model.charset().iter().enumerate() {
            if characters.contains(ch) {
                allowed[i] = true;
                any = true;
            }
        }
        assert!(any, "None of the characters are in the model charset");
        allowed
    }

    pub fn read(&self, bgr: &BgrImage, box_rect: IntRect) -> NameplateRead {
        match NameplateFeatures::create_mask(bgr, box_rect, None, 2, false, None) {
            Some(mask) => self.read_mask(&mask),
            None => NameplateRead::default(),
        }
    }

    pub fn read_mask(&self, mask: &LineMask) -> NameplateRead {
        let (read, _) = self.read_core(
            mask,
            None,
            false,
            NameplateDetectorSettings::NAME_MIN_INK,
            None,
            None,
        );
        read
    }

    pub fn read_pieces(
        &self,
        bgr: &BgrImage,
        box_rect: IntRect,
        allowed: Option<&[bool]>,
        min_ink: f64,
        keep_colors: bool,
    ) -> Option<PieceRead> {
        let mask = NameplateFeatures::create_mask(bgr, box_rect, None, 2, keep_colors, None)?;
        Some(self.read_pieces_from_mask(mask, allowed, min_ink))
    }

    pub fn read_pieces_from_mask(
        &self,
        mask: LineMask,
        allowed: Option<&[bool]>,
        min_ink: f64,
    ) -> PieceRead {
        if let Some(a) = allowed {
            assert_eq!(a.len(), self.model.class_count());
        }
        let mut pieces = Vec::new();
        let mut spans = Vec::new();
        let (_, cap_height) = self.read_core(
            &mask,
            allowed,
            true,
            min_ink,
            Some(&mut pieces),
            Some(&mut spans),
        );
        PieceRead {
            pieces,
            cap_height,
            character_spans: spans,
            mask: Some(mask),
        }
    }

    fn read_core(
        &self,
        m: &LineMask,
        allowed: Option<&[bool]>,
        limit_work: bool,
        min_ink: f64,
        mut pieces: Option<&mut Vec<ReadPiece>>,
        mut spans: Option<&mut Vec<(usize, usize)>>,
    ) -> (NameplateRead, usize) {
        let band = NameplateFeatures::find_cap_band(m);
        let cap = band.height().max(1);
        let cuts = NameplateFeatures::gap_cuts(m);
        if cuts.is_empty() {
            return (NameplateRead::default(), cap);
        }

        let mut text = String::new();
        let mut total = 0.0f64;
        let mut vector = [0.0f64; NameplateFeatures::VECTOR_LENGTH];
        let mut lp = vec![0.0f64; self.model.class_count()];
        let mut scratch = vec![0.0f64; self.model.scratch_size()];
        let min_ink_px = min_ink * (cap as f64) * (cap as f64);

        for (a, b) in cuts {
            let mut ink = 0usize;
            for y in 0..m.height {
                let row = &m.data[y * m.width + a..y * m.width + b.min(m.width)];
                for &v in row {
                    ink += v as usize;
                }
            }
            if (ink as f64) < min_ink_px {
                continue;
            }

            if (b - a) as f64 <= 1.2 * cap as f64 {
                if !NameplateFeatures::glyph_vector(m, a, b, band, &mut vector) {
                    continue;
                }
                self.model
                    .predict_log_probs_with_scratch(&vector, &mut lp, &mut scratch);
                let best = Self::arg_max(&lp, allowed);
                let ch = self.model.charset()[best];
                text.push(ch);
                total += lp[best];
                if let Some(sp) = spans.as_deref_mut() {
                    sp.push((a, b));
                }
                if let Some(ps) = pieces.as_deref_mut() {
                    let free = if allowed.is_none() {
                        best
                    } else {
                        Self::arg_max(&lp, None)
                    };
                    ps.push(ReadPiece {
                        start: a,
                        end: b,
                        text: ch.to_string(),
                        score: lp[best],
                        free_score: lp[free],
                    });
                }
            } else {
                let (piece_text, piece_score) = self.split(
                    m,
                    a,
                    b,
                    band,
                    allowed,
                    limit_work,
                    &mut vector,
                    &mut lp,
                    &mut scratch,
                    spans.as_deref_mut(),
                );
                text.push_str(&piece_text);
                total += piece_score;
                if let Some(ps) = pieces.as_deref_mut() {
                    ps.push(ReadPiece {
                        start: a,
                        end: b,
                        text: piece_text,
                        score: piece_score,
                        free_score: piece_score,
                    });
                }
            }
        }

        (NameplateRead { text, score: total }, cap)
    }

    #[allow(clippy::too_many_arguments)]
    fn split(
        &self,
        m: &LineMask,
        x0: usize,
        x1: usize,
        band: CapBand,
        allowed: Option<&[bool]>,
        limit_work: bool,
        vector: &mut [f64],
        lp: &mut [f64],
        scratch: &mut [f64],
        spans_out: Option<&mut Vec<(usize, usize)>>,
    ) -> (String, f64) {
        let cap = band.height().max(1);
        let width = x1.saturating_sub(x0);
        if width == 0 {
            return (String::new(), Self::SPLIT_FAILURE_SCORE);
        }
        if limit_work && (width as f64) > NameplateDetectorSettings::MAX_PIECE_CAPS * cap as f64 {
            return (String::new(), Self::SPLIT_FAILURE_SCORE);
        }

        let mut col = vec![0usize; width];
        for y in 0..m.height {
            let row = &m.data[y * m.width..];
            for x in 0..width {
                if x0 + x < m.width {
                    col[x] += row[x0 + x] as usize;
                }
            }
        }

        let mut cand = vec![0usize];
        if width > 4 {
            for x in 2..(width - 2) {
                if col[x] <= col[x - 1]
                    && col[x] <= col[x + 1]
                    && (col[x] as f64) <= 0.5 * cap as f64
                {
                    cand.push(x);
                }
            }
        }
        cand.push(width);

        if limit_work {
            let mut pair_count = 0usize;
            for i in 0..cand.len() {
                for j in (i + 1)..cand.len() {
                    let span = (cand[j] - cand[i]) as f64;
                    if span > 1.2 * cap as f64 {
                        break;
                    }
                    if span < 0.25 * cap as f64 {
                        continue;
                    }
                    pair_count += 1;
                }
            }
            if pair_count > NameplateDetectorSettings::MAX_SPLIT_SPANS {
                return (String::new(), Self::SPLIT_FAILURE_SCORE);
            }
        }

        let mut span_from = Vec::new();
        let mut span_to = Vec::new();
        let mut span_char = Vec::new();
        let mut span_logp = Vec::new();

        for i in 0..cand.len() {
            for j in (i + 1)..cand.len() {
                let span = (cand[j] - cand[i]) as f64;
                if span > 1.2 * cap as f64 {
                    break;
                }
                if span < 0.25 * cap as f64 {
                    continue;
                }
                if !NameplateFeatures::glyph_vector(m, x0 + cand[i], x0 + cand[j], band, vector) {
                    continue;
                }
                self.model
                    .predict_log_probs_with_scratch(vector, lp, scratch);
                let best = Self::arg_max(lp, allowed);
                span_from.push(i);
                span_to.push(j);
                span_char.push(best);
                span_logp.push(lp[best]);
            }
        }

        if span_from.is_empty() {
            return (String::new(), Self::SPLIT_FAILURE_SCORE);
        }

        let mut score = vec![-1e9f64; cand.len()];
        let mut back_from = vec![None::<usize>; cand.len()];
        let mut back_char = vec![0usize; cand.len()];
        score[0] = 0.0;

        for k in 0..span_from.len() {
            let from = span_from[k];
            let to = span_to[k];
            let s = score[from] + span_logp[k];
            if score[from] > -1e8 && s > score[to] {
                score[to] = s;
                back_from[to] = Some(from);
                back_char[to] = span_char[k];
            }
        }

        let end = cand.len() - 1;
        if score[end] <= -1e8 {
            return (String::new(), Self::SPLIT_FAILURE_SCORE);
        }

        let mut letters = Vec::new();
        let mut letter_spans = Vec::new();
        let mut at = end;
        while at != 0 {
            let Some(prev) = back_from[at] else {
                break;
            };
            letters.push(self.model.charset()[back_char[at]]);
            letter_spans.push((x0 + cand[prev], x0 + cand[at]));
            at = prev;
        }

        letters.reverse();
        letter_spans.reverse();
        if let Some(sp) = spans_out {
            sp.extend(letter_spans);
        }

        (letters.into_iter().collect(), score[end])
    }

    pub fn is_open_bracket(&self, m: &LineMask, start: usize, end: usize, min_score: f64) -> bool {
        let Some(bracket_idx) = self.model.charset().iter().position(|&c| c == '[') else {
            return false;
        };
        let band = NameplateFeatures::find_cap_band(m);
        let mut vector = [0.0f64; NameplateFeatures::VECTOR_LENGTH];
        if !NameplateFeatures::glyph_vector(m, start, end, band, &mut vector) {
            return false;
        }
        let mut lp = vec![0.0f64; self.model.class_count()];
        self.model.predict_log_probs(&vector, &mut lp);
        let best = Self::arg_max(&lp, None);
        best == bracket_idx && lp[best] >= min_score
    }

    fn arg_max(values: &[f64], allowed: Option<&[bool]>) -> usize {
        let mut best: Option<usize> = None;
        for (i, &v) in values.iter().enumerate() {
            if let Some(a) = allowed {
                if !a[i] {
                    continue;
                }
            }
            if best.map(|b| v > values[b]).unwrap_or(true) {
                best = Some(i);
            }
        }
        best.unwrap_or(0)
    }
}
