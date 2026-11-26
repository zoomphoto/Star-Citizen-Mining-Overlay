use crate::{prepare_display_info_gray_3x, DisplayInfoOcrReader};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::thread;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

struct OcrJob {
    bgr: Vec<u8>,
    width: usize,
    height: usize,
}

/// Non-blocking Windows 11 WinRT OCR reader (`Windows.Media.Ocr.OcrEngine`) running on a
/// dedicated background worker thread so live frame capture never stalls.
pub struct Win32DisplayInfoOcr {
    job_tx: Sender<OcrJob>,
    res_rx: Receiver<String>,
}

impl Win32DisplayInfoOcr {
    pub fn new() -> Self {
        let (job_tx, job_rx) = bounded::<OcrJob>(1);
        let (res_tx, res_rx) = bounded::<String>(4);

        let _ = thread::Builder::new()
            .name("sc-overlay-displayinfo-ocr".to_string())
            .spawn(move || {
                unsafe {
                    let _ = RoInitialize(RO_INIT_MULTITHREADED);
                }
                let engine = match OcrEngine::TryCreateFromUserProfileLanguages() {
                    Ok(e) => e,
                    Err(err) => {
                        tracing::warn!("Windows OCR engine unavailable for r_DisplayInfo: {}", err);
                        return;
                    }
                };

                while let Ok(job) = job_rx.recv() {
                    let (gray_3x, w3, h3) =
                        prepare_display_info_gray_3x(&job.bgr, job.width, job.height);
                    if w3 == 0 || h3 == 0 {
                        continue;
                    }
                    match run_winrt_ocr(&engine, &gray_3x, w3, h3) {
                        Ok(text) => {
                            if !text.trim().is_empty() {
                                let _ = res_tx.try_send(text);
                            }
                        }
                        Err(err) => {
                            tracing::warn!("WinRT OCR failed on r_DisplayInfo crop: {}", err);
                        }
                    }
                }
            });

        Self { job_tx, res_rx }
    }
}

impl Default for Win32DisplayInfoOcr {
    fn default() -> Self {
        Self::new()
    }
}

impl DisplayInfoOcrReader for Win32DisplayInfoOcr {
    fn submit_top_right_bgr(&self, bgr: Vec<u8>, width: usize, height: usize) {
        let _ = self.job_tx.try_send(OcrJob { bgr, width, height });
    }

    fn try_recv_text(&self) -> Option<String> {
        let mut latest = None;
        while let Ok(t) = self.res_rx.try_recv() {
            latest = Some(t);
        }
        latest
    }
}

fn run_winrt_ocr(engine: &OcrEngine, gray: &[u8], w: usize, h: usize) -> windows::core::Result<String> {
    let writer = DataWriter::new()?;
    writer.WriteBytes(gray)?;
    let ibuf = writer.DetachBuffer()?;
    let bmp = SoftwareBitmap::CreateCopyFromBuffer(
        &ibuf,
        BitmapPixelFormat::Gray8,
        w as i32,
        h as i32,
    )?;
    let result = engine.RecognizeAsync(&bmp)?.get()?;
    let lines = result.Lines()?;

    // WinRT OCR often splits a single wide HUD row into separate OcrLines when there is
    // spacing or punctuation. Re-merge words by their vertical center (Y) and sort left-to-right.
    let mut words: Vec<(f32, f32, f32, String)> = Vec::new();
    for line in lines {
        if let Ok(line_words) = line.Words() {
            for word in line_words {
                if let (Ok(rect), Ok(txt)) = (word.BoundingRect(), word.Text()) {
                    let s = txt.to_string();
                    if !s.is_empty() {
                        let cy = rect.Y + rect.Height * 0.5;
                        words.push((cy, rect.X, rect.Height.max(8.0), s));
                    }
                }
            }
        }
    }

    if words.is_empty() {
        return Ok(String::new());
    }

    words.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)));
    let mut rows: Vec<(f32, f32, Vec<(f32, String)>)> = Vec::new();
    for (cy, x, h_px, text) in words {
        if let Some(row) = rows
            .iter_mut()
            .find(|(rcy, rh, _)| (cy - *rcy).abs() <= (*rh).max(h_px) * 0.55)
        {
            row.2.push((x, text));
        } else {
            rows.push((cy, h_px, vec![(x, text)]));
        }
    }

    let mut out = String::new();
    for (_, _, mut row_words) in rows {
        row_words.sort_by(|a, b| a.0.total_cmp(&b.0));
        if !out.is_empty() {
            out.push('\n');
        }
        for (i, (_, t)) in row_words.into_iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            out.push_str(&t);
        }
    }
    Ok(out)
}

